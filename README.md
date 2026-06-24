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
| 4. Optimize Combined score | In progress | 1M hot **0.84s** (0.62× CH); 100M warm **321.843s** vs CH **~32s** @ `80c09f0` — see [`docs/benchmarks/cloud-100m/`](docs/benchmarks/cloud-100m/) · next: [`docs/NEXT.md`](docs/NEXT.md) |
| 5. ClickBench PR | Not started | [`clickbench/coldrun/`](clickbench/coldrun/) — after [`docs/NEXT.md`](docs/NEXT.md) P1 + official `benchmark.sh` |

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

## Cloud dev box (100M `hits`, Jun 2026)

Current perf VM — data already loaded; use for unattended warm + official runs.

| Item | Value |
|------|--------|
| **Instance** | `c6a.4xlarge` (16 vCPU, 32 GiB), Ubuntu 24.04 |
| **SSH** | `ssh -i ~/Downloads/coldrun-bench.pem ubuntu@52.17.231.129` |
| **Repo** | `~/coldrun` — pull `main`, build with `cargo build --release -p coldrun-cli` |
| **Parquet** | `/data/hits.parquet` (~14 GiB) |
| **Coldrun data** | `COLDRUN_DATA=/data/coldrun` — 99,997,497 rows, **27** `.col` + **27** `.blocks.json`, **~14.2 GiB** |
| **ClickHouse** | `./scripts/install-clickhouse-local.sh` (already installed on dev box) |
| **Current warm baseline** | `80c09f0` — all-43 hot **321.843s** (Q23 **56.151s**, Q24 **49.990s**) |

Full checklist + unattended commands: [`docs/CLOUD-RUN.md`](docs/CLOUD-RUN.md)

### Preflight (5 min, on the VM)

```bash
cd ~/coldrun && git pull && cargo build --release -p coldrun-cli
export COLDRUN_ROOT=~/coldrun COLDRUN_DATA=/data/coldrun HITS_PARQUET=/data/hits.parquet

# expect 27 columns, plus 27 block sidecars, ~14.2G
ls /data/coldrun/hits/columns/*.col | wc -l
ls /data/coldrun/hits/columns/*.blocks.json | wc -l
du -sh /data/coldrun

# quick warm smoke (Q1)
./clickbench/coldrun/start && sleep 2
./clickbench/coldrun/query < <(sed -n '1p' clickbench/coldrun/queries.sql)
./clickbench/coldrun/stop
```

### Unattended perf runs (tmux — safe to disconnect)

Run **on the VM** after preflight. Logs under `/data/bench-*.log`.

```bash
# 1) Warm coldrun, all 43 queries, 3 tries (~30–90 min; Q36 is the long pole)
tmux new-session -d -s warm-cr \
  'cd ~/coldrun && export COLDRUN_ROOT=$PWD COLDRUN_DATA=/data/coldrun PATH=$HOME/.cargo/bin:$PATH && \
   ./scripts/bench-serve.sh 100000000 --skip-load 2>&1 | tee /data/bench-warm-coldrun.log'

# 2) Warm ClickHouse on same Parquet + compare (~15–40 min)
tmux new-session -d -s warm-ch \
  'cd ~/coldrun && export PATH=$HOME/.cargo/bin:$PATH && \
   ./scripts/bench-clickhouse-parquet.sh /data/hits.parquet --compare 2>&1 | tee /data/bench-warm-ch.log'

# 3) Official Combined protocol — cold restart + drop_caches per query (~4–8 h)
tmux new-session -d -s official \
  'cd ~/coldrun && export COLDRUN_ROOT=$PWD COLDRUN_DATA=/data/coldrun HITS_PARQUET=/data/hits.parquet && \
   COLDRUN_SKIP_LOAD=1 ./clickbench/coldrun/benchmark.sh 2>&1 | tee /data/bench-official.log'
```

Monitor from your laptop:

```bash
ssh -i ~/Downloads/coldrun-bench.pem ubuntu@52.17.231.129 'tmux ls; tail -3 /data/bench-warm-coldrun.log'
```

Artifacts when done: `logs/benchmarks/serve-last.log`, `clickbench/coldrun/result.csv`, `clickbench/coldrun/clickhouse-result.csv`, `/data/bench-*.log`.

## Next (planned)

**Done on laptop:** demo + 1M Parquet **43/43** vs ClickHouse; warm-serve hot sum **0.84s** vs CH **1.34s** (**0.62×**) — [`compare-hot.md`](docs/benchmarks/parquet-hits-1m/compare-hot.md).

**Done on cloud:** full warm all-43 snapshot @ `80c09f0` (hot **321.843s**), with V2 blockized gains confirmed for Q23/Q24.  
**In flight:** P2 outliers (Q36, Q41, Q33–35), then same-VM ClickHouse compare + official `benchmark.sh` for Combined + ClickBench PR.

Snapshots: [`serve-hot.md`](docs/benchmarks/demo-100k/serve-hot.md) (demo) · [`compare-hot.md`](docs/benchmarks/parquet-hits-1m/compare-hot.md) (1M Parquet) · per-query notes [`docs/perf/`](docs/perf/)

## Out of scope

This is a learning exercise, not a product. We are not building:

- Production reliability, HA, or full SQL standard coverage
- A replacement for ClickHouse, DuckDB, or any real OLAP system
- Anything you should take seriously

## License

[0BSD](LICENSE) — do whatever you want with the code; no attribution required. That does not mean you should run this in production.
