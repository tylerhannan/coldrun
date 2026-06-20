//! Column-chunk fused filter + COUNT GROUP BY (no bool mask, raw slice scans).

use ahash::AHashMap;

use super::agg_heap::top_counts_u128_key;
use super::column_slice::{self, IntCols};
use super::mask_util::{for_each_selected, mask_is_sparse, selected_indices};
use super::simd_scan::for_each_q41_zone_match;

const CHUNK: usize = 8192;
const Q41_PARALLEL_THRESHOLD: usize = 250_000;
const COUNT_SHARDS: usize = 256;

#[inline(always)]
pub fn pack_clientip_quad(ip: i32) -> u128 {
    let u = ip as u32;
    let w1 = u.wrapping_sub(1) as u128;
    let w2 = u.wrapping_sub(2) as u128;
    let w3 = u.wrapping_sub(3) as u128;
    u as u128 | (w1 << 32) | (w2 << 64) | (w3 << 96)
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

type ShardMaps = [AHashMap<u128, u64>; COUNT_SHARDS];

#[inline]
fn empty_shards(cap_hint: usize) -> ShardMaps {
    let cap = (cap_hint / (COUNT_SHARDS * 8)).max(4);
    std::array::from_fn(|_| AHashMap::with_capacity(cap))
}

#[inline]
fn merge_shard_maps(mut a: ShardMaps, mut b: ShardMaps) -> ShardMaps {
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

fn merge_global_topk(
    candidates: impl Iterator<Item = (u128, u64)>,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    top_counts_u128_key(
        candidates.map(|(k, c)| (c, k, (k, c))),
        limit,
        offset,
    )
    .into_iter()
    .map(|(k, c)| (k, c))
    .collect()
}

fn topk_from_shards(shards: ShardMaps, limit: usize, offset: usize) -> Vec<(u128, u64)> {
    let need = limit.saturating_add(offset);
    if need == 0 {
        return Vec::new();
    }
    let mut candidates = Vec::with_capacity(need.saturating_mul(COUNT_SHARDS));
    for m in shards {
        if m.is_empty() {
            continue;
        }
        if m.len() <= need {
            candidates.extend(m);
        } else {
            candidates.extend(top_counts_u128_key(
                m.into_iter().map(|(k, c)| (c, k, (k, c))),
                need,
                0,
            ));
        }
    }
    merge_global_topk(candidates.into_iter(), limit, offset)
}

/// Q36: sort ClientIP + run-length counts — no multi-million-entry hash map.
pub fn clientip_quad_topk(ips: &[i32], limit: usize, offset: usize) -> Vec<(u128, u64)> {
    use super::agg_sort::sorted_topk_i32;
    sorted_topk_i32(ips, limit, offset)
        .into_iter()
        .map(|(ip, c)| (pack_clientip_quad(ip as i32), c))
        .collect()
}

#[inline(always)]
fn pack_q41_pair(url: i64, date: i32, url_high: bool) -> u128 {
    if url_high {
        ((url as u128) << 64) | (date as u32 as u128)
    } else {
        ((date as i64 as u128) << 64) | (url as u64 as u128)
    }
}

/// Q41 fast path: URLHash + EventDate with inlined filters (no IntCols dispatch).
pub fn dashboard_q41_topk(
    zone_ranges: &[(usize, usize)],
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
    url_hashes: &[i64],
    url_high: bool,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    if row_count >= Q41_PARALLEL_THRESHOLD {
        dashboard_q41_topk_parallel(
            zone_ranges,
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
            url_hashes,
            url_high,
            limit,
            offset,
        )
    } else {
        dashboard_q41_topk_serial(
            zone_ranges,
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
            url_hashes,
            url_high,
            limit,
            offset,
        )
    }
}

fn zone_subranges(zone_ranges: &[(usize, usize)], chunk: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &(start, end) in zone_ranges {
        let mut s = start;
        while s < end {
            let e = (s + chunk).min(end);
            out.push((s, e));
            s = e;
        }
    }
    out
}

fn dashboard_q41_topk_serial(
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
    url_hashes: &[i64],
    url_high: bool,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    let keys = collect_q41_keys(
        zone_ranges,
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
        url_hashes,
        url_high,
    );
    super::agg_sort::sorted_topk_u128(&keys, limit, offset)
}

fn dashboard_q41_topk_parallel(
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
    url_hashes: &[i64],
    url_high: bool,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    let keys = collect_q41_keys(
        zone_ranges,
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
        url_hashes,
        url_high,
    );
    super::agg_sort::sorted_topk_u128(&keys, limit, offset)
}

fn collect_q41_keys(
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
    url_hashes: &[i64],
    url_high: bool,
) -> Vec<u128> {
    use rayon::prelude::*;
    let subranges = zone_subranges(zone_ranges, CHUNK);
    subranges
        .par_iter()
        .map(|&(start, end)| {
            let mut keys = Vec::new();
            for_each_q41_zone_match(
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
                |i| keys.push(pack_q41_pair(url_hashes[i], dates[i], url_high)),
            );
            keys
        })
        .reduce(Vec::new, |mut a, mut b| {
            a.append(&mut b);
            a
        })
}

/// Q41 fallback when zone index is unavailable.
pub fn mask_selected_pair_topk(
    mask: &[bool],
    row_count: usize,
    ic1: IntCols<'_>,
    ic2: IntCols<'_>,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    if mask_is_sparse(mask) && row_count >= Q41_PARALLEL_THRESHOLD {
        mask_selected_pair_topk_parallel(mask, ic1, ic2, limit, offset)
    } else {
        mask_selected_pair_topk_serial(mask, row_count, ic1, ic2, limit, offset)
    }
}

fn mask_selected_pair_topk_serial(
    mask: &[bool],
    row_count: usize,
    ic1: IntCols<'_>,
    ic2: IntCols<'_>,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    let mut shards = empty_shards(row_count);
    for_each_selected(mask, row_count, |i| {
        shard_add(&mut shards, column_slice::pack_pair(ic1, ic2, i));
    });
    topk_from_shards(shards, limit, offset)
}

fn mask_selected_pair_topk_parallel(
    mask: &[bool],
    ic1: IntCols<'_>,
    ic2: IntCols<'_>,
    limit: usize,
    offset: usize,
) -> Vec<(u128, u64)> {
    use rayon::prelude::*;

    let indices = selected_indices(mask);
    let cap = (indices.len() / (COUNT_SHARDS * 2)).max(4);
    let shards = indices
        .par_chunks(CHUNK)
        .fold(
            || empty_shards(indices.len()),
            |mut shards, chunk| {
                for &i in chunk {
                    shard_add(&mut shards, column_slice::pack_pair(ic1, ic2, i));
                }
                shards
            },
        )
        .reduce(
            || std::array::from_fn(|_| AHashMap::with_capacity(cap)),
            merge_shard_maps,
        );

    topk_from_shards(shards, limit, offset)
}
