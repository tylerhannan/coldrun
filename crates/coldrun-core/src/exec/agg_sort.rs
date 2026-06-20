//! Sort + run-length GROUP BY counts — O(n log n) but no giant hash maps.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

const SORT_PARALLEL_THRESHOLD: usize = 250_000;

/// Top groups by count from a multiset of keys (sort, scan runs, min-heap of size limit+offset).
pub fn topk_from_sorted_keys<K: Copy + Ord>(sorted: &[K], limit: usize, offset: usize) -> Vec<(K, u64)> {
    let need = limit.saturating_add(offset);
    if need == 0 || sorted.is_empty() {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, K)>> = BinaryHeap::new();
    let mut i = 0;
    while i < sorted.len() {
        let key = sorted[i];
        let mut count = 1u64;
        i += 1;
        while i < sorted.len() && sorted[i] == key {
            count += 1;
            i += 1;
        }
        push_run(&mut heap, count, key, need);
    }
    let mut pairs: Vec<(u64, K)> = heap.into_iter().map(|Reverse(p)| p).collect();
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    pairs
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(c, k)| (k, c))
        .collect()
}

#[inline]
fn push_run<K: Ord>(heap: &mut BinaryHeap<Reverse<(u64, K)>>, count: u64, key: K, need: usize) {
    if heap.len() < need {
        heap.push(Reverse((count, key)));
    } else if let Some(&Reverse((min_c, _))) = heap.peek() {
        if count > min_c {
            heap.push(Reverse((count, key)));
            if heap.len() > need {
                heap.pop();
            }
        }
    }
}

pub fn sorted_topk_i32(ips: &[i32], limit: usize, offset: usize) -> Vec<(u32, u64)> {
    use rayon::prelude::*;
    let mut v: Vec<u32> = if ips.len() >= SORT_PARALLEL_THRESHOLD {
        ips.par_iter().map(|&ip| ip as u32).collect()
    } else {
        ips.iter().map(|&ip| ip as u32).collect()
    };
    if v.len() >= SORT_PARALLEL_THRESHOLD {
        v.par_sort_unstable();
    } else {
        v.sort_unstable();
    }
    topk_from_sorted_keys(&v, limit, offset)
}

pub fn sorted_topk_u128(keys: &[u128], limit: usize, offset: usize) -> Vec<(u128, u64)> {
    let v = copy_and_sort(keys);
    topk_from_sorted_keys(&v, limit, offset)
}

fn copy_and_sort<K: Copy + Ord + Send + Sync>(keys: &[K]) -> Vec<K> {
    use rayon::prelude::*;
    let mut v: Vec<K> = if keys.len() >= SORT_PARALLEL_THRESHOLD {
        keys.par_iter().copied().collect()
    } else {
        keys.to_vec()
    };
    if v.len() >= SORT_PARALLEL_THRESHOLD {
        v.par_sort_unstable();
    } else {
        v.sort_unstable();
    }
    v
}

/// Sort `(user, minute, phrase_hash)` triples and count runs — for Q19.
pub fn sorted_topk_user_minute_phrase(
    pairs: &mut [(i64, i64, u64)],
    limit: usize,
    offset: usize,
) -> Vec<((i64, i64, u64), u64)> {
    use rayon::prelude::*;
    if pairs.len() >= SORT_PARALLEL_THRESHOLD {
        pairs.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    } else {
        pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    }
    let need = limit.saturating_add(offset);
    if need == 0 || pairs.is_empty() {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, i64, i64, u64)>> = BinaryHeap::new();
    let mut i = 0;
    while i < pairs.len() {
        let (user, minute, hash) = pairs[i];
        let mut count = 1u64;
        i += 1;
        while i < pairs.len()
            && pairs[i].0 == user
            && pairs[i].1 == minute
            && pairs[i].2 == hash
        {
            count += 1;
            i += 1;
        }
        let entry = (count, user, minute, hash);
        if heap.len() < need {
            heap.push(Reverse(entry));
        } else if let Some(&Reverse((min_c, _, _, _))) = heap.peek() {
            if count > min_c {
                heap.push(Reverse(entry));
                if heap.len() > need {
                    heap.pop();
                }
            }
        }
    }
    let mut out: Vec<(u64, i64, i64, u64)> = heap.into_iter().map(|Reverse(t)| t).collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3))));
    out.into_iter()
        .skip(offset)
        .take(limit)
        .map(|(c, u, m, h)| ((u, m, h), c))
        .collect()
}

/// Sort `(user, phrase_hash)` pairs and count runs — for Q17/Q18.
pub fn sorted_topk_user_phrase(
    pairs: &mut [(i64, u64)],
    limit: usize,
    offset: usize,
) -> Vec<((i64, u64), u64)> {
    use rayon::prelude::*;
    if pairs.len() >= SORT_PARALLEL_THRESHOLD {
        pairs.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    } else {
        pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    let need = limit.saturating_add(offset);
    if need == 0 || pairs.is_empty() {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, i64, u64)>> = BinaryHeap::new();
    let mut i = 0;
    while i < pairs.len() {
        let (user, hash) = pairs[i];
        let mut count = 1u64;
        i += 1;
        while i < pairs.len() && pairs[i].0 == user && pairs[i].1 == hash {
            count += 1;
            i += 1;
        }
        let entry = (count, user, hash);
        if heap.len() < need {
            heap.push(Reverse(entry));
        } else if let Some(&Reverse((min_c, _, _))) = heap.peek() {
            if count > min_c {
                heap.push(Reverse(entry));
                if heap.len() > need {
                    heap.pop();
                }
            }
        }
    }
    let mut out: Vec<(u64, i64, u64)> = heap.into_iter().map(|Reverse(t)| t).collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1).then(a.2.cmp(&b.2))));
    out.into_iter()
        .skip(offset)
        .take(limit)
        .map(|(c, u, h)| ((u, h), c))
        .collect()
}

/// Distinct count per utf8 key (by hash), sorted `(hash, user)` input.
pub fn distinct_count_per_hash_sorted(
    pairs: &mut [(u64, i64)],
) -> Vec<(u64, u64)> {
    use rayon::prelude::*;
    if pairs.is_empty() {
        return Vec::new();
    }
    if pairs.len() >= SORT_PARALLEL_THRESHOLD {
        pairs.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    } else {
        pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let h = pairs[i].0;
        let mut distinct = 1u64;
        let mut prev = pairs[i].1;
        i += 1;
        while i < pairs.len() && pairs[i].0 == h {
            if pairs[i].1 != prev {
                distinct += 1;
                prev = pairs[i].1;
            }
            i += 1;
        }
        out.push((h, distinct));
    }
    out
}

#[allow(dead_code)]
pub fn cmp_u128(a: u128, b: u128) -> Ordering {
    a.cmp(&b)
}
