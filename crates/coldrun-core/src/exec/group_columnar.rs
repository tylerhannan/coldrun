//! Column-chunk fused filter + COUNT GROUP BY (no bool mask, raw slice scans).

use ahash::AHashMap;

use super::column_slice::{self, IntCols};

pub const COUNT_SHARDS: usize = 256;
const CHUNK: usize = 8192;

pub type ShardMaps = [AHashMap<u128, u64>; COUNT_SHARDS];

#[inline]
pub fn pack_clientip_quad(ip: i32) -> u128 {
    let ip = ip as u32;
    (ip as u128)
        | ((ip.wrapping_sub(1)) as u128) << 32
        | ((ip.wrapping_sub(2)) as u128) << 64
        | ((ip.wrapping_sub(3)) as u128) << 96
}

#[inline]
pub fn unpack_clientip_quad(key: u128) -> [i32; 4] {
    [
        key as u32 as i32,
        (key >> 32) as u32 as i32,
        (key >> 64) as u32 as i32,
        (key >> 96) as u32 as i32,
    ]
}

#[inline]
fn empty_shards(cap_hint: usize) -> ShardMaps {
    let cap = (cap_hint / (COUNT_SHARDS * 8)).max(4);
    std::array::from_fn(|_| AHashMap::with_capacity(cap))
}

#[inline]
pub fn merge_shards(mut a: ShardMaps, mut b: ShardMaps) -> ShardMaps {
    for i in 0..COUNT_SHARDS {
        for (k, v) in b[i].drain() {
            *a[i].entry(k).or_insert(0) += v;
        }
    }
    a
}

#[inline]
fn shard_add(shards: &mut ShardMaps, key: u128) {
    let shard = (key as usize) % COUNT_SHARDS;
    *shards[shard].entry(key).or_insert(0) += 1;
}

/// Q36: scan contiguous ClientIP column in cache-friendly chunks.
pub fn clientip_quad_count(ips: &[i32]) -> ShardMaps {
    let mut shards = empty_shards(ips.len());
    for chunk in ips.chunks(CHUNK) {
        for &ip in chunk {
            shard_add(&mut shards, pack_clientip_quad(ip));
        }
    }
    shards
}

/// Q41: referer-equality-led column chunks, then remaining dashboard preds.
pub fn columnar_referer_pair_count(
    row_count: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    ic1: IntCols<'_>,
    ic2: IntCols<'_>,
) -> ShardMaps {
    let mut shards = empty_shards(row_count);
    let mut i = 0usize;
    while i < row_count {
        let end = (i + CHUNK).min(row_count);
        let mut j = i;
        while j < end {
            if referer[j] != referer_hash {
                j += 1;
                continue;
            }
            if counters[j] != counter {
                j += 1;
                continue;
            }
            let d = dates[j];
            if d < min_date || d > max_date {
                j += 1;
                continue;
            }
            if refresh[j] != is_refresh {
                j += 1;
                continue;
            }
            let t = traffic[j];
            if t != -1 && t != 6 {
                j += 1;
                continue;
            }
            let key = column_slice::pack_pair(ic1, ic2, j);
            shard_add(&mut shards, key);
            j += 1;
        }
        i = end;
    }
    shards
}

/// Zone-pruned dashboard ranges (used when referer is not selective).
pub fn columnar_dashboard_pair_count(
    row_count: usize,
    zone_ranges: &[(usize, usize)],
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    ic1: IntCols<'_>,
    ic2: IntCols<'_>,
) -> ShardMaps {
    let mut shards = empty_shards(row_count);
    for &(start, end) in zone_ranges {
        scan_dashboard_pair_range(
            &mut shards,
            start,
            end,
            referer_hash,
            counter,
            min_date,
            max_date,
            is_refresh,
            referer,
            counters,
            dates,
            refresh,
            traffic,
            ic1,
            ic2,
        );
    }
    shards
}

#[inline]
fn scan_dashboard_pair_range(
    shards: &mut ShardMaps,
    start: usize,
    end: usize,
    referer_hash: i64,
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer: &[i64],
    counters: &[i32],
    dates: &[i32],
    refresh: &[i16],
    traffic: &[i16],
    ic1: IntCols<'_>,
    ic2: IntCols<'_>,
) {
    let mut i = start;
    while i < end {
        let block_end = (i + CHUNK).min(end);
        while i < block_end {
            if referer[i] != referer_hash {
                i += 1;
                continue;
            }
            if counters[i] != counter {
                i += 1;
                continue;
            }
            let d = dates[i];
            if d < min_date || d > max_date {
                i += 1;
                continue;
            }
            if refresh[i] != is_refresh {
                i += 1;
                continue;
            }
            let t = traffic[i];
            if t != -1 && t != 6 {
                i += 1;
                continue;
            }
            let key = column_slice::pack_pair(ic1, ic2, i);
            shard_add(shards, key);
            i += 1;
        }
    }
}
