//! Streaming top-K by count — prune hash map when group count explodes.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::hash::Hash;

use ahash::AHashMap;

use super::agg_heap::top_counts;

/// Incremental group counts with optional prune before materializing all keys.
pub struct StreamingTopK<K: Hash + Eq + Clone> {
    counts: AHashMap<K, u64>,
    limit: usize,
    offset: usize,
    prune_factor: usize,
}

impl<K: Hash + Eq + Clone + Ord> StreamingTopK<K> {
    pub fn new(limit: usize, offset: usize) -> Self {
        Self::with_prune_factor(limit, offset, 64)
    }

    pub fn with_prune_factor(limit: usize, offset: usize, prune_factor: usize) -> Self {
        Self {
            counts: AHashMap::new(),
            limit,
            offset,
            prune_factor: prune_factor.max(4),
        }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.counts.contains_key(key)
    }

    pub fn inc(&mut self, key: K) {
        *self.counts.entry(key).or_insert(0) += 1;
        self.maybe_prune();
    }

    fn maybe_prune(&mut self) {
        let need = self.limit.saturating_add(self.offset);
        if need > 0 && self.counts.len() > need.saturating_mul(self.prune_factor) {
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

    /// Top `(key, count)` pairs by count descending (for a second aggregation pass).
    pub fn top_entries(self) -> Vec<(K, u64)> {
        let mut pairs: Vec<(u64, K)> = self.counts.into_iter().map(|(k, c)| (c, k)).collect();
        pairs.sort_by(|a, b| b.0.cmp(&a.0));
        pairs
            .into_iter()
            .skip(self.offset)
            .take(self.limit)
            .map(|(c, k)| (k, c))
            .collect()
    }

    pub fn finish<T, F>(self, mut row: F) -> Vec<T>
    where
        F: FnMut(K, u64) -> T,
        T: Clone,
    {
        let scored = self.counts.into_iter().map(|(k, c)| (c, row(k, c)));
        top_counts(scored, self.limit, self.offset)
    }

    /// Like [`finish`](Self::finish), but break count ties with `tie_key` (lexicographic ascending).
    pub fn finish_with_tie_key<T, F, G>(self, mut row: F, mut tie_key: G) -> Vec<T>
    where
        F: FnMut(K, u64) -> T,
        G: FnMut(&K) -> String,
        T: Clone,
    {
        let need = self.limit.saturating_add(self.offset);
        if need == 0 {
            return Vec::new();
        }
        // Min-heap on (count, Reverse(tie)) so peek evicts the worst kept row: lowest count,
        // then largest tie key among ties (ORDER BY count DESC, tie ASC).
        let mut heap: BinaryHeap<Reverse<(u64, Reverse<String>, usize)>> = BinaryHeap::new();
        let mut storage: Vec<T> = Vec::new();

        for (k, c) in self.counts {
            let tk = tie_key(&k);
            if heap.len() < need {
                let idx = storage.len();
                storage.push(row(k, c));
                heap.push(Reverse((c, Reverse(tk), idx)));
            } else if let Some(&Reverse((min_c, Reverse(ref max_tk), _))) = heap.peek() {
                if c > min_c || (c == min_c && tk < *max_tk) {
                    let idx = storage.len();
                    storage.push(row(k, c));
                    heap.push(Reverse((c, Reverse(tk), idx)));
                    if heap.len() > need {
                        heap.pop();
                    }
                }
            }
        }

        let mut pairs: Vec<(u64, String, T)> = heap
            .into_iter()
            .map(|Reverse((c, Reverse(tk), i))| (c, tk, storage[i].clone()))
            .collect();
        pairs.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
        });
        pairs
            .into_iter()
            .skip(self.offset)
            .take(self.limit)
            .map(|(_, _, t)| t)
            .collect()
    }
}
