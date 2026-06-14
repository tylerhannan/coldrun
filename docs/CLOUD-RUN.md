# Cloud run checklist (ClickBench full `hits`)

Use this before the first **100M-row** run on AWS. Laptop 1M numbers are for regression only; Combined score needs the full protocol on `c6a.4xlarge`.

## Before you provision

- [ ] Repo at the commit you intend to publish (`git rev-parse HEAD` recorded).
- [ ] `./scripts/smoke-all.sh 1000000` passes locally (optional but fast sanity).
- [ ] `./scripts/validate-parquet.sh data/hits-1m.parquet` — **43/43** vs ClickHouse.
- [ ] `./scripts/cloud-dry-run-local.sh` completes (harness smoke on 1M Parquet).

## VM spec (ClickBench default)

| Item | Value |
|------|--------|
| Instance | **c6a.4xlarge** (16 vCPU, 32 GiB) |
| OS | Ubuntu **24.04** LTS |
| Disk | **500 GB gp2**, mount at `/data` |
| Region | Any; keep instance + disk in same AZ |

Stretch goal only: document separately if you also run `c6a.metal`.

## One-time VM setup

```bash
# SSH into fresh VM
sudo apt-get update
sudo apt-get install -y build-essential curl git pkg-config

# Rust (install script also runs this if missing)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
source "$HOME/.cargo/env"

# Dataset (~15 GB download)
sudo mkdir -p /data
sudo chown "$USER:$USER" /data
cd /data
curl -LO https://datasets.clickhouse.com/hits_compatible/hits.parquet
ls -lh hits.parquet   # expect ~15G
```

## Clone and build

```bash
git clone https://github.com/tylerhannan/coldrun.git
cd coldrun
git checkout <your-branch-or-tag>

export COLDRUN_ROOT="$PWD"
export HITS_PARQUET=/data/hits.parquet
export COLDRUN_DATA=/data/coldrun

./clickbench/coldrun/install          # cargo build --release
./target/release/coldrun --version    # sanity
```

## Load (measure load time + on-disk size)

```bash
time ./clickbench/coldrun/load
COLDRUN_DATA=/data/coldrun ./clickbench/coldrun/data-size
```

Record **Load time** and **Data size** — both count toward Combined (10% each).

## Correctness spot-check (recommended)

ClickHouse on the same Parquet file (install `clickhouse-local` or use upstream binary):

```bash
# From repo on VM — copy hits-1m slice or validate subset via HTTP/local
# Full 43 on 100M is slow; at minimum run Q1, Q36, Q41 via query helper:
COLDRUN_DATA=/data/coldrun ./clickbench/coldrun/start
sleep 2
./clickbench/coldrun/query < <(sed -n '1p' clickbench/coldrun/queries.sql)
./clickbench/coldrun/query < <(sed -n '36p' clickbench/coldrun/queries.sql)
./clickbench/coldrun/query < <(sed -n '41p' clickbench/coldrun/queries.sql)
./clickbench/coldrun/stop
```

For full parity vs ClickHouse on a slice, use `./scripts/validate-parquet.sh` on a 1M sample before scaling.

## Official benchmark (Combined protocol)

```bash
export COLDRUN_ROOT="$PWD"
export HITS_PARQUET=/data/hits.parquet
export COLDRUN_DATA=/data/coldrun

# True cold: restart + drop_caches per query (slow; hours on 100M)
./clickbench/coldrun/benchmark.sh | tee logs/benchmarks/cloud-100m.log
```

Expected output shape (for `results/c6a.4xlarge.json`):

```
Load time: <seconds>
Data size: <bytes>
<t1,t2,t3>   # Q1 — cold, hot2, hot3
...
<t1,t2,t3>   # Q43
```

**Hot** = min(try 2, try 3). **Cold** = try 1 after restart + page cache drop (needs `sudo` for `drop_caches` on Linux).

## After the run

- [ ] Save log: `logs/benchmarks/cloud-100m.log`
- [ ] Copy `clickbench/coldrun/result.csv` if generated
- [ ] Note **Q36, Q41, Q19, Q9** hot/cold times (historical pain points)
- [ ] Compare hot sum to laptop 1M snapshot — expect different absolute times, same relative outliers
- [ ] Update `results/c6a.4xlarge.json` when format validated
- [ ] File ClickBench PR per [How To Add a New Result](https://github.com/ClickHouse/ClickBench)

## Competitive rules reminder

- No extra indexes beyond PK; no materialized views / pre-aggregation for the score.
- No query-result caching; buffer pool / OS page cache OK.
- End-to-end timing (client → server → rows returned).
- Single-file load; document if you parallelize ingest.
- Vanilla config for official entry; tuned variants labeled separately.

## Troubleshooting

| Symptom | Check |
|---------|--------|
| `load` OOM | 32 GiB should suffice; ensure nothing else heavy on box |
| `serve` won't start | `COLDRUN_DATA/serve.log`, port 9000 free |
| `benchmark.sh` hangs | `check` script, stale `serve.pid` |
| Cold times flat vs hot | `drop_caches` needs root; see `benchmark-local.sh` |
| Query wrong vs CH | Re-run validate on 1M slice; fix before trusting 100M |

## Local dry-run (no AWS)

```bash
./scripts/cloud-dry-run-local.sh
# Log: logs/benchmarks/cloud-dry-run.log
```

Uses 1M Parquet + embedded warm bench — **not** a Combined score, but exercises `install` → `load` → `start` → `bench-clickbench` → `stop`.
