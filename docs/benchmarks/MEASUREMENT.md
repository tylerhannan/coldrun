# How to measure coldrun

ClickBench **Combined** scores need a warm server, three tries per query, and (for cold) restart + cache drop between the first try and the rest. Demo @100k on a laptop is for **regression**, not leaderboard rank.

## Scripts (pick one)

| Script | Server | Restarts | Tries | Use for |
|--------|--------|----------|-------|---------|
| [`bench-all.sh`](../../scripts/bench-all.sh) | No (new CLI each query) | 43× process | 1 | Fast dev regression; **not** ClickBench hot |
| [`bench-serve.sh`](../../scripts/bench-serve.sh) | Yes (`serve`) | None (warm) | 3 | **Hot-shaped** local timing; subset friendly |
| [`bench-clickbench.sh`](../../scripts/bench-clickbench.sh) `--embedded` | Yes | None | 3 | ClickBench output format, warm (quick) |
| [`bench-clickbench.sh`](../../scripts/bench-clickbench.sh) (default) | Yes | Per query | 3 | Full cold protocol (slow on laptop) |

## Recommended local workflow

```bash
# 1) Quick correctness
./scripts/smoke-all.sh 100000

# 2) Dev regression (CLI-per-query; compare commits)
./scripts/bench-all.sh 100000

# 3) Hot-shaped timing (warm serve, min of tries 2–3)
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
| `docs/benchmarks/demo-100k/latest.md` | Committed `bench-all` snapshot |

## Cloud / real data

When you have a VM and `hits.parquet`:

```bash
./clickbench/coldrun/install
HITS_PARQUET=/data/hits.parquet COLDRUN_DATA=/data/coldrun ./clickbench/coldrun/load
./clickbench/coldrun/benchmark.sh   # upstream driver if present
```

Until then, treat `bench-all` numbers as **relative** on one machine only.
