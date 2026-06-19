# Cloud run checklist (ClickBench full `hits`)

Use this for **100M-row** runs on AWS. Laptop 1M numbers are regression only; Combined score needs the full protocol on `c6a.4xlarge`.

## Current dev box (Jun 2026)

| Item | Value |
|------|--------|
| Instance | **c6a.4xlarge** (16 vCPU, 32 GiB) |
| SSH | `ssh -i ~/Downloads/coldrun-bench.pem ubuntu@34.244.176.182` |
| Repo | `~/coldrun` on `main` |
| Parquet | `/data/hits.parquet` |
| Coldrun data | `COLDRUN_DATA=/data/coldrun` (27 columns under `hits/columns/`, ~33 GiB) |
| `drop_caches` | Passwordless sudo OK (required for official cold protocol) |

IP changes when you stop/start the instance — update README + this doc if you reprovision.

## Before you provision (first time only)

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
sudo apt-get install -y build-essential curl git pkg-config tmux

# Rust
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
git checkout main   # or your publish tag

export COLDRUN_ROOT="$PWD"
export HITS_PARQUET=/data/hits.parquet
export COLDRUN_DATA=/data/coldrun

./clickbench/coldrun/install          # cargo build --release
./target/release/coldrun --version    # sanity
./scripts/install-clickhouse-local.sh # once, for CH compare
```

## Load (measure load time + on-disk size)

Skip if `/data/coldrun` already has 27 `.col` files (~33 GiB).

```bash
time ./clickbench/coldrun/load
COLDRUN_DATA=/data/coldrun ./clickbench/coldrun/data-size
ls /data/coldrun/hits/columns/*.col | wc -l   # expect 27
```

Record **Load time** and **Data size** — both count toward Combined (10% each).

Loader history: streaming staging (`4365385`), u64 UTF8 offsets (`41d2268`), disk-backed offsets + progress (`e134699`).

## Preflight before unattended bench (dev box)

Run on the VM every time you pull new code:

```bash
cd ~/coldrun
git pull
cargo build --release -p coldrun-cli
git rev-parse --short HEAD    # record in log / PR — expect 18d7641+ for Q23 fix

export COLDRUN_ROOT="$PWD"
export COLDRUN_DATA=/data/coldrun
export HITS_PARQUET=/data/hits.parquet

# Data sanity
du -sh /data/coldrun
ls /data/coldrun/hits/columns/*.col | wc -l
COLDRUN_DATA=/data/coldrun ./clickbench/coldrun/data-size

# Warm smoke — Q1 should return ~100M rows in <0.1s
./clickbench/coldrun/start && sleep 2
./clickbench/coldrun/query < <(sed -n '1p' clickbench/coldrun/queries.sql)
./clickbench/coldrun/stop

# Optional spot-check outliers after perf fixes
./clickbench/coldrun/start && sleep 2
for q in 9 36 41; do
  echo "--- Q$q ---"
  ./clickbench/coldrun/query < <(sed -n "${q}p" clickbench/coldrun/queries.sql)
done
./clickbench/coldrun/stop
```

## Unattended perf runs (tmux)

Use **tmux** so SSH disconnect does not kill long jobs. Logs go to `/data/bench-*.log`.

### 1. Warm coldrun (hot-shaped, no per-query restart)

All 43 queries × 3 tries. Expect **~30–90 min** (Q21 URL scan and Q36 sort are long poles).

```bash
tmux new-session -d -s warm-cr \
  'cd ~/coldrun && export COLDRUN_ROOT=$PWD COLDRUN_DATA=/data/coldrun PATH=$HOME/.cargo/bin:$PATH && \
   ./scripts/bench-serve.sh 100000000 --skip-load 2>&1 | tee /data/bench-warm-coldrun.log'
```

Resume from a query after a crash (fresh serve loads new binary):

```bash
tmux new-session -d -s rebench-tail \
  'cd ~/coldrun && export COLDRUN_ROOT=$PWD COLDRUN_DATA=/data/coldrun PATH=$HOME/.cargo/bin:$PATH && \
   ./scripts/bench-serve.sh 100000000 --skip-load --from 23 2>&1 | tee /data/bench-rebench-tail.log'
```

Output: `logs/benchmarks/serve-last.log`, `clickbench/coldrun/result.csv`

### 2. Warm ClickHouse + compare

Same queries on `/data/hits.parquet`. Expect **~15–40 min**.

```bash
tmux new-session -d -s warm-ch \
  'cd ~/coldrun && export PATH=$HOME/.cargo/bin:$PATH && \
   ./scripts/bench-clickhouse-parquet.sh /data/hits.parquet --compare 2>&1 | tee /data/bench-warm-ch.log'
```

Output: `clickbench/coldrun/clickhouse-result.csv`, compare summary on stderr

Run **after** warm-cr finishes if you want sequential load on one box; or run in parallel on a second VM.

### 3. Official Combined protocol (cold + hot)

Restart + `drop_caches` before each query’s first try. Expect **~4–8 hours** on 100M.

```bash
tmux new-session -d -s official \
  'cd ~/coldrun && export COLDRUN_ROOT=$PWD COLDRUN_DATA=/data/coldrun HITS_PARQUET=/data/hits.parquet && \
   COLDRUN_SKIP_LOAD=1 ./clickbench/coldrun/benchmark.sh 2>&1 | tee /data/bench-official.log'
```

Expected output shape (for `results/c6a.4xlarge.json`):

```
Load time: <seconds>   # 0 if COLDRUN_SKIP_LOAD=1; record real load from step above
Data size: <bytes>
<t1,t2,t3>   # Q1 — cold, hot2, hot3
...
<t1,t2,t3>   # Q43
```

**Hot** = min(try 2, try 3). **Cold** = try 1 after restart + page cache drop.

### Monitor from laptop

```bash
ssh -i ~/Downloads/coldrun-bench.pem ubuntu@34.244.176.182 'tmux ls'
ssh -i ~/Downloads/coldrun-bench.pem ubuntu@34.244.176.182 'tail -5 /data/bench-warm-coldrun.log'
ssh -i ~/Downloads/coldrun-bench.pem ubuntu@34.244.176.182 'grep -c "^\[" /data/bench-warm-coldrun.log'  # query lines done
```

Attach to a session: `tmux attach -t warm-cr` (Ctrl+B D to detach).

**Do not** `tmux kill-session` on a running bench unless you intend to stop it. If a session already exists, attach and check progress first.

### Suggested order for publish-ready numbers

1. Preflight (above)
2. **warm-cr** → review `serve-last.log`, note Q9/Q36/Q41
3. **warm-ch** → `--compare` hot sum vs coldrun
4. **official** → save full log for ClickBench PR
5. Copy artifacts off VM before terminating instance

```bash
# From laptop — after runs complete
scp -i ~/Downloads/coldrun-bench.pem \
  ubuntu@34.244.176.182:/data/bench-*.log \
  ubuntu@34.244.176.182:~/coldrun/logs/benchmarks/serve-last.log \
  ubuntu@34.244.176.182:~/coldrun/clickbench/coldrun/result.csv \
  .
```

## After the run

- [ ] Save logs: `/data/bench-*.log`, `logs/benchmarks/serve-last.log`
- [ ] Copy `clickbench/coldrun/result.csv` and `clickhouse-result.csv`
- [ ] Note **Q36, Q41, Q19, Q9** hot/cold times (historical pain points)
- [ ] Record git SHA (`git rev-parse HEAD`) with every published number
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
| `load` OOM | 32 GiB should suffice with streaming loader; ensure nothing else heavy on box |
| `serve` won't start | `COLDRUN_DATA/serve.log`, port 9000 free, `./clickbench/coldrun/stop` |
| `benchmark.sh` hangs | `./check`, stale `serve.pid`, `tmux attach -t official` |
| Cold times flat vs hot | `sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'` must succeed |
| Query wrong vs CH | Re-run validate on 1M slice; fix before trusting 100M |
| Bench dies at Q23 | Pre-fix: OOM — pair materialization or per-phrase sets on ~30 GiB warm cache. Pull latest, rebuild, `--from 23` |
| Serve RSS >25 GiB on Q23 | Warm serve holds all loaded columns; Q23 must use O(distinct groups) agg, not O(rows) sort buffer |
| `ls *.col` count 0 | Columns live under `$COLDRUN_DATA/hits/columns/`, not data root |
| tmux session gone | Job died — check end of `/data/bench-*.log` for error |

## Local dry-run (no AWS)

```bash
./scripts/cloud-dry-run-local.sh
# Log: logs/benchmarks/cloud-dry-run.log
```

Uses 1M Parquet + embedded warm bench — **not** a Combined score, but exercises `install` → `load` → `start` → `bench-clickbench` → `stop`.
