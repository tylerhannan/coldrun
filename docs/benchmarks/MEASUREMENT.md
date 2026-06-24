# How to measure coldrun

ClickBench **Combined** scores need a warm server, three tries per query, and (for cold) restart + cache drop between the first try and the rest. Demo @100k on a laptop is for **regression**, not leaderboard rank.

## Scripts (pick one)

| Script | Server | Restarts | Tries | Use for |
|--------|--------|----------|-------|---------|
| [`bench-all.sh`](../../scripts/bench-all.sh) | No (new CLI each query) | 43× process | 1 | Fast dev regression; **not** ClickBench hot |
| [`bench-serve.sh`](../../scripts/bench-serve.sh) | Yes (`serve`) | None (warm) | 3 | **Hot-shaped** timing; auto-compares to `latest.md` |
| [`bench-clickhouse-parquet.sh`](../../scripts/bench-clickhouse-parquet.sh) | No (`local --time`) | Per try | 3 | ClickHouse `file()` Parquet hot snapshot + compare |
| [`bench-clickbench.sh`](../../scripts/bench-clickbench.sh) `--embedded` | Yes | None | 3 | ClickBench output format, warm (quick) |
| [`bench-clickbench.sh`](../../scripts/bench-clickbench.sh) (default) | Yes | Per query | 3 | Full cold protocol (slow on laptop) |

## Recommended local workflow

```bash
# 1) Quick correctness
./scripts/smoke-all.sh 100000

# 2) Dev regression (CLI-per-query; compare commits)
./scripts/bench-all.sh 100000

# 3) Hot-shaped timing (warm serve, min of tries 2–3)
./scripts/bench-serve.sh 100000 --write-snapshot   # refresh serve-hot.md
./scripts/bench-serve.sh 100000 --from 1 --to 10
./scripts/bench-serve.sh 100000 --queries 6,23,40

# 4) See CLI spawn tax on Q1
./scripts/bench-serve.sh 100000 --compare-only

# 5) Full ClickBench-shaped run (when you have time)
./scripts/bench-clickbench.sh --demo 100000 --embedded
```

Logs (gitignored): `logs/benchmarks/serve-last.log`, `clickbench-last.log`.

## Hot vs cold (ClickBench)

- **Hot** (60% of Combined): `min(try2, try3)` with a **warm** server and OS page cache.
- **Cold** (20%): first try after restart + `drop_caches` (Linux + sudo; no-op on macOS).

`bench-serve.sh` prints a **hot summary** on stderr (`min` of tries 2–3). It does **not** simulate cold.

## Artifacts

| File | Contents |
|------|----------|
| `clickbench/coldrun/result.csv` | `num,try,seconds` per run |
| `docs/benchmarks/demo-100k/latest.md` | Committed `bench-all` snapshot (CLI per query) |
| `docs/benchmarks/demo-100k/serve-hot.md` | Committed `bench-serve` hot snapshot @ 100k demo |
| `docs/benchmarks/parquet-hits-1m/serve-hot.md` | Committed `bench-serve` hot snapshot @ 1M Parquet |
| `docs/benchmarks/parquet-hits-1m/clickhouse-hot.md` | Committed ClickHouse `file()` hot snapshot @ 1M Parquet |
| `docs/benchmarks/parquet-hits-1m/compare-hot.md` | Side-by-side coldrun vs ClickHouse hot sum + per-query |
| `docs/benchmarks/parquet/validation-1m.md` | 43/43 ClickHouse validation log summary |

## Real data without AWS

If you have `hits.parquet` (or a slice) on disk:

| Step | Script |
|------|--------|
| Slice | [`sample-parquet.sh`](../../scripts/sample-parquet.sh) `hits.parquet 1000000 hits-1m.parquet` |
| Correctness | [`validate-parquet.sh`](../../scripts/validate-parquet.sh) `hits-1m.parquet` |
| Validate + bench | [`measure-parquet.sh`](../../scripts/measure-parquet.sh) `hits-1m.parquet` |

Requires **ClickHouse** in [`clickhouse-local/`](../../clickhouse-local/) (`./scripts/install-clickhouse-local.sh`). Validation compares coldrun output to ClickHouse on the same Parquet file. Details: [`parquet/README.md`](parquet/README.md).

**1M snapshot (warm serve, hot sum):** coldrun **0.84s** vs ClickHouse **1.34s** on the same slice (**0.62×**). See [`compare-hot.md`](parquet-hits-1m/compare-hot.md).

When you regenerate [`parquet-hits-1m/serve-hot.md`](parquet-hits-1m/serve-hot.md) or [`clickhouse-hot.md`](parquet-hits-1m/clickhouse-hot.md), also refresh [`compare-hot.md`](parquet-hits-1m/compare-hot.md) and summary numbers in [`README.md`](../../README.md), [`PERF.md`](../../PERF.md), [`parquet/README.md`](parquet/README.md), and [`ARCHITECTURE.md`](../../ARCHITECTURE.md).

## Cloud (when available)

```bash
./clickbench/coldrun/install
HITS_PARQUET=/data/hits.parquet COLDRUN_DATA=/data/coldrun ./clickbench/coldrun/load
./clickbench/coldrun/benchmark.sh
```

Until you have real Parquet locally, treat demo `bench-all` / `bench-serve` numbers as **relative** on one machine only.

## Iteration tiers (when to use 100M)

**Not every perf change needs a full 100M VM run.** Use the cheapest tier that can falsify your hypothesis; promote to 100M only for scale-sensitive work.

| Tier | Where | Rows | Time | Use for |
|------|-------|------|------|---------|
| **1** | Laptop | 100k demo | seconds | `smoke-all`, `bench-all`, `bench-serve --queries N` — correctness, fast-path wiring |
| **2** | Laptop | 1M Parquet slice | minutes | `validate-parquet.sh`, `measure-parquet.sh`, `bench-serve` @ 1M — real utf8/GROUP BY, CH compare |
| **3** | VM (tmux) | 1M–10M sample | tens of min | `sample-parquet.sh` + load + `bench-serve` — cloud warm-serve without 33 GiB load |
| **4** | VM (tmux) | **100M** | hours | OOM, full LZ4 scan cost, `cloud-100m/` snapshots, ClickBench Combined |

**Requires 100M (tier 4):**

- Q23/Q24 absolute hot time (dominated by full-column decompress @ ~100M)
- Memory / OOM fixes on 32 GiB (`c6a.4xlarge`)
- Parallelism / concurrency tuning (rayon batch size — can regress only at scale)
- Updating [`cloud-100m/serve-hot.md`](cloud-100m/serve-hot.md) or official Combined

**Tier 1–2 is enough for:** dispatch logic, correctness, most GROUP BY / sort paths, 1M hot regression.

**Tier 3** optional middle ground on the bench VM:

```bash
# tmux run on the cloud VM
./scripts/sample-parquet.sh /data/hits.parquet 10000000 /data/hits-10m.parquet
COLDRUN_DATA=/data/coldrun-10m ./clickbench/coldrun/load
./scripts/bench-serve.sh 10000000 --skip-load --from 23 --to 24
```

**Tier 4** single-query while iterating an outlier (tmux + tee):

```bash
./scripts/bench-serve.sh 100000000 --skip-load --from 24 --to 24
```

Full 43-query re-bench only at milestones — see [`NEXT.md`](../NEXT.md) P0.3.

