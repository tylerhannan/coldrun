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
