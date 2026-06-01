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

/// Top groups by count desc, breaking ties with ascending `first_seen` (scan order).
pub fn top_counts_first_seen<T: Clone>(
    items: impl Iterator<Item = (u64, u32, T)>,
    limit: usize,
    offset: usize,
) -> Vec<T> {
    let need = limit.saturating_add(offset);
    if need == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, Reverse<u32>, usize)>> = BinaryHeap::new();
    let mut storage: Vec<T> = Vec::new();

    for (count, first_seen, item) in items {
        if heap.len() < need {
            let idx = storage.len();
            storage.push(item);
            heap.push(Reverse((count, Reverse(first_seen), idx)));
        } else if let Some(&Reverse((min_c, Reverse(min_fs), _))) = heap.peek() {
            if count > min_c || (count == min_c && first_seen < min_fs) {
                let idx = storage.len();
                storage.push(item);
                heap.push(Reverse((count, Reverse(first_seen), idx)));
                if heap.len() > need {
                    heap.pop();
                }
            }
        }
    }

    let mut pairs: Vec<(u64, u32, T)> = heap
        .into_iter()
        .map(|Reverse((c, Reverse(fs), i))| (c, fs, storage[i].clone()))
        .collect();
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    pairs
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, _, t)| t)
        .collect()
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

/// Collect `(count, row)` in scan order, then unstable sort by count desc (ties keep scan order).
pub fn top_counts_scan_order<T: Clone>(
    items: impl Iterator<Item = (u64, T)>,
    limit: usize,
    offset: usize,
) -> Vec<T> {
    let mut pairs: Vec<(u64, T)> = items.collect();
    pairs.sort_by(|a, b| b.0.cmp(&a.0));
    pairs
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, t)| t)
        .collect()
}
