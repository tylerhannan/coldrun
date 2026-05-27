# bench-all @ 100k — pass 2 (streaming top-K + fused kernels)

**Command:** `./scripts/bench-all.sh 100000`

## Headline wins vs pass 1 (fused)

| Q | pass 1 | pass 2 | Speedup |
|---|--------|--------|---------|
| 12 | 0.100 | **0.023** | 4.3× |
| 31 | 0.088 | **0.019** | 4.6× |
| 32 | 0.091 | **0.019** | 4.8× |
| 33 | 0.128 | **0.024** | 5.3× |
| 11 | 0.083 | **0.023** | 3.6× |
| 24 | 0.042 | **0.022** | 1.9× |

## Still >10ms (adversarial)

| Q | ms | Why not 1ms? |
|---|-----|----------------|
| 19 | 131 | 100k hash inserts (UserID×minute×phrase) |
| 36 | 116 | ~unique 4-tuples on demo data |
| 35 | 93 | URL string hashing |

## New machinery

- `agg_heap.rs` — O(n log k) top-K by count, not full sort
- `for_each_selected` — no `Vec<usize>` on dense masks
- `try_fused_int16_utf8_distinct` — Q12
- Q1 `row_count` metadata; Q2 nonzero column count
