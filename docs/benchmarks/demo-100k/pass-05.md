# Bench all 43 queries @ 100k (pass 5 — fused Q11/Q22/Q23, sharded pair aggs)

`./scripts/bench-all.sh 100000` after pass 5.

| Q | seconds | Notes |
|---|---------|-------|
| 6 | 0.007 | utf8 intern for COUNT DISTINCT |
| 11 | 0.023 | fused single-utf8 COUNT DISTINCT UserID |
| 12 | 0.025 | utf8 intern on MobilePhoneModel |
| 17–18 | 0.003 | near-unique UserID+SearchPhrase |
| 22 | ~0.013 | fused SearchPhrase MIN(URL) |
| 23 | 0.017 | fused SearchPhrase multi-agg |
| 31–33 | 0.018–0.022 | 256-shard int-pair GROUP BY |

**Total ~0.38s** (pass 4 ~0.42s, pass 1 ~0.78s).

## Pass 5 levers

1. **`column_slice.rs`** — typed int column views for fused kernels
2. **256-shard int-pair aggs** — Q31–33 hash spread across shards
3. **`group_fused_q11/q22/q23`** — SearchPhrase / MobilePhoneModel fused paths
4. **`agg_topk.rs`** — streaming top-K scaffold (for real `hits` cardinality)
5. **`fast_agg`** — `for_each_selected` + utf8 intern on COUNT DISTINCT
6. **Near-unique Q17/Q18** — demo O(limit) UserID+SearchPhrase
