# Smoke demo

Fast local check that Coldrun builds and runs [ClickBench](https://benchmark.clickhouse.com/) queries **1–15** on synthetic `hits` data.

**You do not need `hits.parquet` on this machine.** The full ClickBench file is ~15 GB; use these scripts for everyday development. Reserve the real dataset for a cloud box or when you care about benchmark scores.

For all **43** queries (pass/fail report), use [`scripts/smoke-all.sh`](../scripts/smoke-all.sh).

## Prerequisites

- **Rust stable** with `cargo` on your `PATH` ([rustup](https://rustup.rs/) is recommended).
- If a terminal says `cargo: command not found` after installing Rust, reload the shell or run:

  ```bash
  source "$HOME/.cargo/env"
  ```

## Run

From the repository root:

```bash
./scripts/smoke-demo.sh
```

Optional: pass a row count (default `10000`):

```bash
./scripts/smoke-demo.sh 50000
```

Optional: use a different data directory:

```bash
COLDRUN_DATA=/tmp/coldrun-demo ./scripts/smoke-demo.sh
```

## What the script does

1. `cargo build --release -p coldrun-cli` → `target/release/coldrun`
2. Removes and recreates the data dir (default `.coldrun-demo/`)
3. Loads synthetic `hits` rows: `coldrun local --demo <rows>`
4. Runs queries 1–15 from [`clickbench/coldrun/queries.sql`](../clickbench/coldrun/queries.sql), printing the last few lines of each run (result + timing)

First run may take a minute while dependencies compile; later runs are much faster.

## Example output

```
=== ClickBench queries 1–15 (demo, 10000 rows) ===
>> Q1: SELECT COUNT(*) FROM hits;
count()
10000
0.042
>> Q2: ...
```

The last number on each query block is elapsed time in seconds.

## Manual equivalent

```bash
cargo build --release -p coldrun-cli
export PATH="$PWD/target/release:$PATH"   # or use ./target/release/coldrun directly

./target/release/coldrun --data-dir .coldrun-demo local --demo 10000
./target/release/coldrun --data-dir .coldrun-demo local --sql "SELECT COUNT(*) FROM hits"
```

## Related scripts

| Script | Purpose |
|--------|---------|
| [`scripts/smoke-demo.sh`](../scripts/smoke-demo.sh) | Queries 1–10 on demo data (this doc) |
| [`scripts/smoke-all.sh`](../scripts/smoke-all.sh) | All 43 queries (pass/fail; work in progress) |
| [`scripts/repro-local.sh`](../scripts/repro-local.sh) | Load real `hits.parquet` and smoke |

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `cargo: command not found` | `source "$HOME/.cargo/env"` or open a new terminal after `rustup` install |
| Permission denied on script | `chmod +x scripts/smoke-demo.sh` (should already be executable in git) |
| Query errors on Q16+ in smoke-demo | Use `smoke-all.sh` for the full query set |
