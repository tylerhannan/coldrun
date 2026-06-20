//! Keep only top groups by COUNT — avoid sorting millions of rows.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Retain the `limit + offset` largest `count` values; return sorted desc.
pub fn top_counts<T: Clone>(items: impl Iterator<Item = (u64, T)>, limit: usize, offset: usize) -> Vec<T> {
    let need = limit.saturating_add(offset);
    if need == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
    let mut storage: Vec<T> = Vec::new();

    for (count, item) in items {
        if heap.len() < need {
            let idx = storage.len();
            storage.push(item);
            heap.push(Reverse((count, idx)));
        } else if let Some(&Reverse((min_c, _))) = heap.peek() {
            if count > min_c {
                let idx = storage.len();
                storage.push(item);
                heap.push(Reverse((count, idx)));
                if heap.len() > need {
                    heap.pop();
                }
            }
        }
    }

    let mut pairs: Vec<(u64, T)> = heap
        .into_iter()
        .map(|Reverse((c, i))| (c, storage[i].clone()))
        .collect();
    pairs.sort_by(|a, b| b.0.cmp(&a.0));
    pairs.into_iter().skip(offset).take(limit).map(|(_, t)| t).collect()
}

/// Top groups by count desc; ties break with ascending packed int pair ([`column_slice::cmp_packed_pair`]).
pub fn top_counts_u128_key<T: Clone>(
    items: impl Iterator<Item = (u64, u128, T)>,
    limit: usize,
    offset: usize,
) -> Vec<T> {
    use super::column_slice::{cmp_packed_pair, PackedPairKey};

    let need = limit.saturating_add(offset);
    if need == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, Reverse<PackedPairKey>, usize)>> = BinaryHeap::new();
    let mut storage: Vec<T> = Vec::new();

    for (count, key, item) in items {
        if heap.len() < need {
            let idx = storage.len();
            storage.push(item);
            heap.push(Reverse((count, Reverse(PackedPairKey(key)), idx)));
        } else if let Some(&Reverse((min_c, Reverse(PackedPairKey(max_key)), _))) = heap.peek() {
            if count > min_c
                || (count == min_c && cmp_packed_pair(key, max_key) == std::cmp::Ordering::Less)
            {
                let idx = storage.len();
                storage.push(item);
                heap.push(Reverse((count, Reverse(PackedPairKey(key)), idx)));
                if heap.len() > need {
                    heap.pop();
                }
            }
        }
    }

    let mut pairs: Vec<(u64, u128, T)> = heap
        .into_iter()
        .map(|Reverse((c, Reverse(PackedPairKey(k)), i))| (c, k, storage[i].clone()))
        .collect();
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| cmp_packed_pair(a.1, b.1)));
    pairs
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, _, t)| t)
        .collect()
}
