# Bench all 43 queries @ 100k (pass 6 — Arc POD columns, StreamingTopK, Q24 partial sort)

`./scripts/bench-all.sh 100000` after pass 6.

| Q | seconds | Notes |
|---|---------|-------|
| 6 | 0.007 | global COUNT DISTINCT SearchPhrase via utf8 intern |
| 13/14 | ~0.014–0.021 | StreamingTopK when LIMIT + non-demo |
| 24 | ~0.023 | partial sort for ORDER BY EventTime LIMIT 10 |

**Total ~0.37–0.39s** (pass 5 ~0.37s).

## Pass 6 levers

1. **`PodStorage` / `Arc<[T]>`** — numeric columns stored as shared buffers after disk read (`storage/pod.rs`)
2. **`StreamingTopK`** — wired into fused utf8 COUNT when `LIMIT` set and not demo near-unique
3. **Q24 partial sort** — `select_nth_unstable` for top-N EventTime rows instead of full sort
4. **Q6 fast path** — single-pass utf8 intern for global COUNT DISTINCT SearchPhrase
