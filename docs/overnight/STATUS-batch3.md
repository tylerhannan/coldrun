# Batch 3 status (perf items 18–21)

Completed on demo data (`ROWS=100000` unless noted).

| # | Item | Status |
|---|------|--------|
| 18 | Referer host GROUP BY + HAVING shortcut (Q28–29) | Done |
| 19 | 4-key int GROUP BY with `col - N` (Q36) | Done |
| 20 | ClickBench harness README | Done |
| 21 | `bench-compare.sh` before/after | Done |

## Notes

- **Q28/Q29**: `HAVING COUNT(*) > 100000` on 100k demo returns immediately (zero groups).
- **Q29** (when groups exist): `group_referrer.rs` groups by cached host string without per-row `eval_group_key` allocation.
- **Q36**: `group_int.rs` packs four `ClientIP` arithmetic keys in one `u128`.
- **bench-compare**: first run saves `.coldrun-bench-all/baseline-{ROWS}.tsv`; second run diffs.

Correctness: `./scripts/smoke-all.sh` → 43/43 PASS.

## bench-all @ 100k (post batch 3)

See [`bench-all-100k.md`](bench-all-100k.md). Highlights:

| Q | batch 2 | batch 3 |
|---|---------|---------|
| 29 | 0.182s | 0.004s |
| 28 | — | 0.004s |
| 36 | 0.243s | 0.122s |

Slowest remaining: Q32 (0.144s), Q33 (0.142s), Q19 (0.139s).
