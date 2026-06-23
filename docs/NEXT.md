# Next steps (prioritized)

**Baseline:** warm-serve 100M on `c6a.4xlarge` @ [`eb414c9`](https://github.com/tylerhannan/coldrun/commit/eb414c9) — see [`benchmarks/cloud-100m/`](benchmarks/cloud-100m/) and [`PERF.md`](PERF.md).

**Goal:** shrink the **~674s** hot sum (all 43) toward ClickHouse **~32s**, without regressing 1M correctness (**43/43** vs CH) or warm-serve stability on 32 GiB.

**How to bench:** tiered workflow in [`benchmarks/MEASUREMENT.md`](benchmarks/MEASUREMENT.md#iteration-tiers-when-to-use-100m) — most iterations on laptop 1M; 100M VM only for scale-sensitive milestones (tmux required on cloud).

**Workflow (every step):** smoke/validate locally → **commit + push to `main`** → tmux bench on VM (if scale-sensitive) → update docs + **commit + push** again with bench results.

## Acceleration strategy (2026-06 reset)

Before more micro-tuning, prioritize structural changes that can produce order-of-magnitude gains:

1. **Reliability first:** fix Q23 "silent exit" and force explicit error output in bench logs before any further perf claims.
2. **Instrument phases:** add per-phase timings + bytes-decompressed counters for Q23/Q24 (mask, count, top-k, pass2, projection). ✅ Implemented in core fast paths (`perf:q23`, `perf:q24` stderr lines).
3. **Stop wide parallel decode:** avoid rayon over full-column LZ4 work; only parallelize bounded/sparse work.
4. **Blockized read path (main lever):** add block metadata + block iterators so LIKE/filter/top-k can run block-at-a-time without full-column decompress.
5. **Apply in order:** Q24 first (best stress case for late materialization), then Q23, then propagate pattern to Q21/Q22/Q36/Q41.
6. **Benchmark discipline:** isolated single-query runs are diagnostic; canonical numbers come from full warm runs and documented snapshots.

Target trajectory: Q23/Q24 **<120s** first milestone, then **<60s**, then tail-sum reduction before Combined submission.

### Non-negotiable directives (owner: this project)

1. **Build a blockized column read path (highest leverage).**
   - Add V2 on-disk metadata/sidecar with fixed-size row blocks (target 64k rows).
   - Track per-block compressed boundaries and expose cheap `read_block(block_id)` / `iter_blocks()`.
   - Keep V1 fallback compatibility for existing `.col` files.
   - Goal: never decompress full 100M columns when only a subset of blocks is needed.

2. **Rewrite Q24 and Q23 to consume block readers.**
   - Q24: URL LIKE scan block-by-block, maintain top-k row ids by EventTime, late-materialize final rows/cols.
   - Q23: block mask + block phrase count + block batched pass2; avoid full-column scans on sparse masks.
   - Expected impact: move from ~200s class toward tens of seconds.

3. **Add phase-level perf accounting (must-have).**
   - For Q23/Q24 log: bytes decompressed per column, blocks read, rows tested, rows materialized, phase timings.
   - This is required before trusting isolated benchmark deltas.

4. **Apply same blockized pattern to Q21/Q22/Q36/Q41.**
   - These are string-heavy and decode-bound; reuse the same infra after Q23/Q24.

5. **Tighten benchmarking protocol.**
   - Canonical numbers come only from full warm runs + committed snapshots.
   - Isolated single-query runs are diagnostic only.
   - Run same-VM ClickHouse compare (`P6.1`) once warm runs stabilize.

---

## P0 — Hygiene (do first)

| # | Item | Why | Action |
|---|------|-----|--------|
| 0.1 | ~~**Merge warning cleanup**~~ | Clean build signal before perf work | Done — merged [PR #1](https://github.com/tylerhannan/coldrun/pull/1) @ `118e60d` |
| 0.2 | ~~**Formal Q23 bench**~~ | Only smoke (~234s); skews totals | Done — hot **222.341s** @ `dde9184` ([238.8, 229.9, 222.3]); log `/data/bench-q23-fix3.log` |
| 0.3 | **Re-bench after each P1 fix** | Hot = min(try 2, 3); update [`cloud-100m/serve-hot.md`](benchmarks/cloud-100m/serve-hot.md) | tmux + `./scripts/bench-serve.sh 100000000 --skip-load --write-snapshot` — see [`CLOUD-RUN.md`](CLOUD-RUN.md) |

---

## P1 — Outliers (~458s of ~674s; fix these first)

Full-column utf8 decode on 100M rows dominates. Same fix class: **scan compressed bytes block-at-a-time** (LIKE / empty checks) and **project only needed cells** (sidecar `.col.idx` already exists).

| # | Query | CR hot | CH hot | Work | Code |
|---|-------|--------|--------|------|------|
| 1.1 | **Q24** | 231s | 0.10s | **Follow-up:** block-at-a-time URL scan + cell-at projection without full LZ4 expand (see below) | [`scan_stream.rs`](../crates/coldrun-core/src/exec/scan_stream.rs), [`table.rs`](../crates/coldrun-core/src/storage/table.rs) |
| 1.2 | **Q23** | 222s | 0.61s | In progress: stable OOM-safe pass2 (row-slot map) shipped; next is block-at-a-time decode to push below 120s | [`group_fused_q23.rs`](../crates/coldrun-core/src/exec/group_fused_q23.rs) |

**Success target:** each ≪ **60s** on warm serve (stretch: ≪ **10s**).

Detail: [`perf/q-23.md`](perf/q-23.md), [`perf/q-24.md`](perf/q-24.md).

### P1 follow-up — failed parallel attempt (`6b64ee7`, reverted)

| Item | Result |
|------|--------|
| **Change** | Rayon row-range URL/EventTime top-K + 4-wide parallel `project_rows` |
| **Bench** | Formal 3-try @ 100M — tries [352.9, 280.6, 262.3], hot **262.3s** |
| **vs baseline** | **231.3s** @ `eb414c9` — **+13% regression** (likely LZ4 memory/CPU contention) |
| **Log** | `/data/bench-q24-formal.log` on bench VM |
| **Action** | Reverted parallel paths; real win needs **streaming decode** (not more rayon on full-column LZ4) |

### P1 follow-up — revert verify (`2419ade`)

| Item | Result |
|------|--------|
| **Change** | Restore sequential URL/EventTime top-K + sequential `project_rows` |
| **Bench** | Isolated 3-try @ 100M (tmux `bench-q24-verify`) — tries [262.0, 257.9, 281.4], hot **257.854s** |
| **vs baseline** | **231.3s** @ `eb414c9` — still **~+11%** on isolated re-bench |
| **Log** | `/data/bench-q24-verify.log` on bench VM (`52.17.231.129`) |
| **Note** | Canonical Q24 in [`serve-hot.md`](benchmarks/cloud-100m/serve-hot.md) stays **231.3s** (from full warm run); isolated re-bench may differ (cache/VM state). Full warm re-bench if numbers diverge again. |

---

## P2 — High ratio, large Δ (tail Q25–43)

Excluding Q23/Q24, Q25–43 sum is **~170s** vs CH **~23s**.

| # | Query | CR hot | CH hot | Work | Code |
|---|-------|--------|--------|------|------|
| 2.1 | **Q36** | 83s | 0.25s | Fused `REGEXP_REPLACE(Referer, …)` host extract on stream; avoid materializing full Referer | [`group_columnar.rs`](../crates/coldrun-core/src/exec/group_columnar.rs), [`q-36.md`](perf/q-36.md) |
| 2.2 | **Q41** | 7.5s | 0.013s | Tighten zone + sort path; single-pass 5-col dashboard GROUP BY without repeated string decode | [`group_columnar.rs`](../crates/coldrun-core/src/exec/group_columnar.rs), [`q-41.md`](perf/q-41.md) |
| 2.3 | **Q33–35** | ~15–17s | ~3s | Multi-column utf8/int GROUP BY — extend columnar shard pattern from Q31–32 | [`group_fused.rs`](../crates/coldrun-core/src/exec/group_fused.rs), [`column_slice.rs`](../crates/coldrun-core/src/storage/column_slice.rs) |

---

## P3 — String GROUP BY (Q1–22 band)

| # | Query | CR hot | CH hot | Work |
|---|-------|--------|--------|------|
| 3.1 | **Q22** | 4.9s | 0.09s | SearchPhrase GROUP BY — fused path exists; still full phrase decode |
| 3.2 | **Q21** | 4.5s | 0.31s | URL GROUP BY after LIKE filter |
| 3.3 | **Q14** | 9.2s | 0.75s | Sort-based distinct done; still ~12× — reduce phrase/UserID materialization |

---

## P4 — Dashboard LIKE cluster (Q37–43)

Many queries at **80–190×** ratio but **~3–4s** each absolute. Shared pattern: dashboard zone mask + **Referer/URL string predicates** on cold utf8 columns.

| # | Item | Work |
|---|------|------|
| 4.1 | **Q37–40, Q42–43** | One shared streaming Referer/URL matcher over mmap’d column bytes (same infrastructure as P1) |
| 4.2 | **Q40** | CASE on referer host — keep fused kernel, feed it block scans |

---

## P5 — Remaining Q1–22 gaps (ratio > 5×, Δ > 0.5s)

Not blockers for tail sum but worth batching after P1–P2:

| Query | CR | CH | Notes |
|-------|-----|-----|-------|
| Q5 | 2.7s | 0.27s | Global COUNT DISTINCT SearchPhrase |
| Q10 | 4.4s | 0.49s | AdvEngineID GROUP BY |
| Q13 | 3.2s | 0.53s | SearchPhrase COUNT |
| Q16 | 3.1s | 0.38s | UserID GROUP BY |
| Q7–8 | ~0.25s | ~0.01s | Simple int GROUP BY — low absolute Δ |

---

## P6 — Measurement & publication

| # | Item | Notes |
|---|------|-------|
| 6.1 | **ClickHouse on same VM** | `./scripts/bench-clickhouse-parquet.sh /data/hits.parquet --write-snapshot --compare` — apples-to-apples vs CR warm serve |
| 6.2 | **Official Combined** | `clickbench/coldrun/benchmark.sh` + `drop_caches` per query (~4–8 h on c6a.4xlarge) |
| 6.3 | **ClickBench PR** | Submit `results/c6a.4xlarge.json` after Combined + stable warm path — [`clickbench/coldrun/`](../clickbench/coldrun/) |
| 6.4 | **Update README step 4** | Keep [`README.md`](../README.md) build table in sync after next cloud snapshot |

Runbook: [`CLOUD-RUN.md`](CLOUD-RUN.md).

---

## Wins to preserve

Do not regress:

- **Q25** — CR **0.008s** vs CH 0.038s (column-order scan)
- **Q29** — CR **7.9s** vs CH 9.6s (referer host fused GROUP BY)
- **1M hot sum** — **0.84s** (0.62× CH) — run `./scripts/measure-parquet.sh data/hits-1m.parquet` before merging large exec changes

---

## Suggested order of execution

Each bullet: **implement → smoke/validate → commit + push → tmux bench (if 100M) → docs → commit + push**.

1. P0.1 → P1.1 (Q24) → re-bench → P1.2 (Q23) → re-bench  
2. P2.1 (Q36) → P2.2 (Q41) → P2.3 (Q33–35)  
3. P4.1 (shared string scan) — unlocks P3 and most of P4 in one pass  
4. P6.1–6.3 when warm sum is within ~2–5× of CH on tail queries  

When an item ships, update [`PERF.md`](PERF.md) changelog and the relevant [`perf/q-*.md`](perf/) note, then **commit + push**.
