//! Bump arena for utf8 GROUP BY keys — one allocation per unique string.

use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHasher};

/// Intern strings into a single buffer; count by stable id.
pub struct Utf8CountArena {
    buf: Vec<u8>,
    spans: Vec<(u32, u32)>,
    hash_to_id: AHashMap<u64, u32>,
    counts: AHashMap<u32, u64>,
}

impl Utf8CountArena {
    pub fn with_capacity(keys: usize) -> Self {
        Self {
            buf: Vec::with_capacity(keys * 16),
            spans: Vec::with_capacity(keys),
            hash_to_id: AHashMap::with_capacity(keys),
            counts: AHashMap::with_capacity(keys),
        }
    }

    pub fn add(&mut self, s: &str) {
        let id = self.intern(s);
        *self.counts.entry(id).or_insert(0) += 1;
    }

    pub fn into_rows(self) -> Vec<(u64, String)> {
        let mut out = Vec::with_capacity(self.counts.len());
        for (id, count) in self.counts {
            let (off, len) = self.spans[id as usize];
            let s = std::str::from_utf8(&self.buf[off as usize..off as usize + len as usize])
                .unwrap_or("")
                .to_string();
            out.push((count, s));
        }
        out
    }

    fn intern(&mut self, s: &str) -> u32 {
        let h = hash_str(s);
        if let Some(&id) = self.hash_to_id.get(&h) {
            let (off, len) = self.spans[id as usize];
            if self.buf.get(off as usize..off as usize + len as usize) == Some(s.as_bytes()) {
                return id;
            }
        }
        let off = self.buf.len() as u32;
        let bytes = s.as_bytes();
        self.buf.extend_from_slice(bytes);
        let id = self.spans.len() as u32;
        self.spans.push((off, bytes.len() as u32));
        self.hash_to_id.insert(h, id);
        id
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h = AHasher::default();
    s.hash(&mut h);
    h.finish()
}
