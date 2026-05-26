//! Streaming top-K by count — prune hash map when group count explodes.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::hash::Hash;

use ahash::AHashMap;

use super::agg_heap::top_counts;

/// Incremental group counts with optional prune before materializing all keys.
/// Scaffold for Parquet / real `hits` where group cardinality ≫ LIMIT.
pub struct StreamingTopK<K: Hash + Eq + Clone> {
    counts: AHashMap<K, u64>,
    limit: usize,
    offset: usize,
}

impl<K: Hash + Eq + Clone + Ord> StreamingTopK<K> {
    pub fn new(limit: usize, offset: usize) -> Self {
        Self {
            counts: AHashMap::new(),
            limit,
            offset,
        }
    }

    pub fn inc(&mut self, key: K) {
        *self.counts.entry(key).or_insert(0) += 1;
        let need = self.limit.saturating_add(self.offset);
        if need > 0 && self.counts.len() > need.saturating_mul(64) {
            self.prune();
        }
    }

    fn prune(&mut self) {
        let need = self.limit.saturating_add(self.offset);
        if need == 0 {
            return;
        }
        let mut heap: BinaryHeap<Reverse<(u64, K)>> = BinaryHeap::new();
        for (k, c) in self.counts.drain() {
            if heap.len() < need {
                heap.push(Reverse((c, k)));
            } else if let Some(&Reverse((min_c, _))) = heap.peek() {
                if c > min_c {
                    heap.push(Reverse((c, k)));
                    if heap.len() > need {
                        heap.pop();
                    }
                }
            }
        }
        for Reverse((c, k)) in heap {
            self.counts.insert(k, c);
        }
    }

    pub fn finish<T, F>(self, mut row: F) -> Vec<T>
    where
        F: FnMut(K, u64) -> T,
        T: Clone,
    {
        let scored = self.counts.into_iter().map(|(k, c)| (c, row(k, c)));
        top_counts(scored, self.limit, self.offset)
    }
}
