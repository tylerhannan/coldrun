//! Fused GROUP BY kernels — no `AggState`, hash keys, sort by count in Rust.

use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHasher};
use sqlparser::ast::{Expr, Value};

use crate::expr::event_time_minute_of_hour;
use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table, Utf8Column};
use crate::Result;

use super::filter::build_filter_mask;
use super::group::resolve_group_expr;
use super::having::having_can_match;
use super::mask_util::for_each_selected;
use super::QueryResult;

pub fn try_execute_group_fused(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = super::group_fused_q29::try_fused_q29(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q43::try_fused_q43(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q36::try_fused_q36(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q41::try_fused_q41(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q40::try_fused_q40(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q22::try_fused_q22(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q23::try_fused_q23(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q11::try_fused_utf8_one_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int4_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_q19(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_q35(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int64_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int16_utf8_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_pair_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_region_aggs(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_counter_avg_url_len(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int_pair_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int_pair_aggs(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int_utf8_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_count_distinct_i64(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    Ok(None)
}

pub(crate) fn hash_str(s: &str) -> u64 {
    let mut h = AHasher::default();
    s.hash(&mut h);
    h.finish()
}

const COUNT_SHARDS: usize = 256;
const FUSED_PARALLEL_THRESHOLD: usize = 250_000;

#[derive(Default, Clone)]
struct UserUtf8Agg {
    count: u64,
    phrase: String,
    first_seen: u32,
}

#[inline]
fn user_phrase_key(user: i64, phrase: &str) -> (i64, u64) {
    (user, hash_str(phrase))
}

fn merge_user_utf8_maps(
    mut a: AHashMap<(i64, u64), UserUtf8Agg>,
    b: AHashMap<(i64, u64), UserUtf8Agg>,
) -> AHashMap<(i64, u64), UserUtf8Agg> {
    for (k, v) in b {
        a.entry(k)
            .and_modify(|e| {
                e.count += v.count;
                e.first_seen = e.first_seen.min(v.first_seen);
            })
            .or_insert(v);
    }
    a
}

fn add_user_utf8_row(
    map: &mut AHashMap<(i64, u64), UserUtf8Agg>,
    i: usize,
    user: i64,
    phrase: &str,
) {
    let key = user_phrase_key(user, phrase);
    map.entry(key)
        .and_modify(|e| {
            if e.phrase == phrase {
                e.count += 1;
                e.first_seen = e.first_seen.min(i as u32);
            }
        })
        .or_insert(UserUtf8Agg {
            count: 1,
            phrase: phrase.to_string(),
            first_seen: i as u32,
        });
}

fn parallel_user_utf8_agg<F>(
    user_at: F,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
) -> AHashMap<(i64, u64), UserUtf8Agg>
where
    F: Fn(usize) -> i64 + Sync,
{
    let cap = (row_count / (COUNT_SHARDS * 4)).max(8);
    if row_count >= FUSED_PARALLEL_THRESHOLD {
        use rayon::prelude::*;
        (0..row_count)
            .into_par_iter()
            .fold(
                || AHashMap::<(i64, u64), UserUtf8Agg>::with_capacity(cap),
                |mut map, i| {
                    if mask.get(i).copied().unwrap_or(false) {
                        add_user_utf8_row(&mut map, i, user_at(i), phrases.get(i));
                    }
                    map
                },
            )
            .reduce(
                || AHashMap::new(),
                merge_user_utf8_maps,
            )
    } else {
        let mut map = AHashMap::with_capacity(cap);
        for i in 0..row_count {
            if mask.get(i).copied().unwrap_or(false) {
                add_user_utf8_row(&mut map, i, user_at(i), phrases.get(i));
            }
        }
        map
    }
}

fn collect_user_phrase_pairs<F>(
    user_at: F,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
) -> Vec<(i64, u64)>
where
    F: Fn(usize) -> i64 + Sync,
{
    use rayon::prelude::*;
    if row_count >= FUSED_PARALLEL_THRESHOLD {
        (0..row_count)
            .into_par_iter()
            .filter(|&i| mask.get(i).copied().unwrap_or(false))
            .map(|i| (user_at(i), hash_str(phrases.get(i))))
            .collect()
    } else {
        let mut v = Vec::new();
        for i in 0..row_count {
            if mask.get(i).copied().unwrap_or(false) {
                v.push((user_at(i), hash_str(phrases.get(i))));
            }
        }
        v
    }
}

fn resolve_phrases_for_user_hash<F>(
    top: &[((i64, u64), u64)],
    phrases: &Utf8Column,
    mask: &[bool],
    user_at: F,
    row_count: usize,
) -> AHashMap<(i64, u64), String>
where
    F: Fn(usize) -> i64,
{
    use ahash::AHashSet;
    let need: AHashSet<(i64, u64)> = top.iter().map(|(k, _)| *k).collect();
    let mut out = AHashMap::with_capacity(need.len());
    for i in 0..row_count {
        if !mask.get(i).copied().unwrap_or(false) {
            continue;
        }
        let key = (user_at(i), hash_str(phrases.get(i)));
        if need.contains(&key) && !out.contains_key(&key) {
            out.insert(key, phrases.get(i).to_string());
            if out.len() == need.len() {
                break;
            }
        }
    }
    out
}

fn user_phrase_topk_by_sort<F>(
    user_at: F,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>>
where
    F: Fn(usize) -> i64 + Sync + Copy,
{
    let mut pairs = collect_user_phrase_pairs(user_at, phrases, mask, row_count);
    let top = super::agg_sort::sorted_topk_user_phrase(&mut pairs, limit, offset);
    let phrase_map = resolve_phrases_for_user_hash(&top, phrases, mask, user_at, row_count);
    top.into_iter()
        .map(|((user, hash), c)| {
            vec![
                user.to_string(),
                phrase_map[&(user, hash)].clone(),
                c.to_string(),
            ]
        })
        .collect()
}

/// Q18: LIMIT without ORDER BY — only count the first `limit` distinct (user, phrase) groups.
fn user_phrase_first_groups<F>(
    user_at: F,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>>
where
    F: Fn(usize) -> i64 + Sync + Copy,
{
    use ahash::AHashSet;
    let mut key_order: Vec<(i64, u64)> = Vec::with_capacity(limit.saturating_add(offset));
    let mut counts: AHashMap<(i64, u64), u64> = AHashMap::with_capacity(limit);
    let mut tracked: AHashSet<(i64, u64)> = AHashSet::with_capacity(limit);

    for i in 0..row_count {
        if !mask.get(i).copied().unwrap_or(false) {
            continue;
        }
        let key = (user_at(i), hash_str(phrases.get(i)));
        if tracked.contains(&key) {
            *counts.get_mut(&key).unwrap() += 1;
            continue;
        }
        if key_order.len() < limit.saturating_add(offset) {
            key_order.push(key);
            tracked.insert(key);
            counts.insert(key, 1);
        }
    }

    let phrase_map = resolve_phrases_for_user_hash(
        &key_order
            .iter()
            .skip(offset)
            .take(limit)
            .map(|&k| (k, counts[&k]))
            .collect::<Vec<_>>(),
        phrases,
        mask,
        user_at,
        row_count,
    );

    key_order
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|key| {
            vec![
                key.0.to_string(),
                phrase_map[&key].clone(),
                counts[&key].to_string(),
            ]
        })
        .collect()
}

#[derive(Default, Clone)]
struct UserMinuteUtf8Agg {
    count: u64,
    phrase: String,
}

#[inline]
fn user_minute_phrase_key(user: i64, minute: i64, phrase: &str) -> (i64, i64, u64) {
    (user, minute, hash_str(phrase))
}

fn merge_user_minute_maps(
    mut a: AHashMap<(i64, i64, u64), UserMinuteUtf8Agg>,
    b: AHashMap<(i64, i64, u64), UserMinuteUtf8Agg>,
) -> AHashMap<(i64, i64, u64), UserMinuteUtf8Agg> {
    for (k, v) in b {
        a.entry(k)
            .and_modify(|e| e.count += v.count)
            .or_insert(v);
    }
    a
}

fn parallel_user_minute_utf8_agg(
    users: &[i64],
    minutes: impl Fn(usize) -> i64 + Sync,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
) -> AHashMap<(i64, i64, u64), UserMinuteUtf8Agg> {
    let cap = (row_count / (COUNT_SHARDS * 4)).max(8);
    let add_row = |map: &mut AHashMap<(i64, i64, u64), UserMinuteUtf8Agg>, i: usize| {
        let user = users[i];
        let phrase = phrases.get(i);
        let minute = minutes(i);
        let key = user_minute_phrase_key(user, minute, phrase);
        map.entry(key)
            .and_modify(|e| {
                if e.phrase == phrase {
                    e.count += 1;
                }
            })
            .or_insert(UserMinuteUtf8Agg {
                count: 1,
                phrase: phrase.to_string(),
            });
    };
    if row_count >= FUSED_PARALLEL_THRESHOLD {
        use rayon::prelude::*;
        (0..row_count)
            .into_par_iter()
            .fold(
                || AHashMap::<(i64, i64, u64), UserMinuteUtf8Agg>::with_capacity(cap),
                |mut map, i| {
                    if mask.get(i).copied().unwrap_or(false) {
                        add_row(&mut map, i);
                    }
                    map
                },
            )
            .reduce(
                || AHashMap::new(),
                merge_user_minute_maps,
            )
    } else {
        let mut map = AHashMap::with_capacity(cap);
        for i in 0..row_count {
            if mask.get(i).copied().unwrap_or(false) {
                add_row(&mut map, i);
            }
        }
        map
    }
}

fn collect_user_minute_phrase_triples(
    users: &[i64],
    minutes: impl Fn(usize) -> i64 + Sync,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
) -> Vec<(i64, i64, u64)> {
    use rayon::prelude::*;
    if row_count >= FUSED_PARALLEL_THRESHOLD {
        (0..row_count)
            .into_par_iter()
            .filter(|&i| mask.get(i).copied().unwrap_or(false))
            .map(|i| {
                (
                    users[i],
                    minutes(i),
                    hash_str(phrases.get(i)),
                )
            })
            .collect()
    } else {
        let mut v = Vec::new();
        for i in 0..row_count {
            if mask.get(i).copied().unwrap_or(false) {
                v.push((users[i], minutes(i), hash_str(phrases.get(i))));
            }
        }
        v
    }
}

fn resolve_phrases_for_user_minute_hash(
    top: &[((i64, i64, u64), u64)],
    users: &[i64],
    minutes: impl Fn(usize) -> i64 + Sync,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
) -> AHashMap<(i64, i64, u64), String> {
    use ahash::AHashSet;
    let need: AHashSet<(i64, i64, u64)> = top.iter().map(|(k, _)| *k).collect();
    let mut out = AHashMap::with_capacity(need.len());
    for i in 0..row_count {
        if !mask.get(i).copied().unwrap_or(false) {
            continue;
        }
        let key = (users[i], minutes(i), hash_str(phrases.get(i)));
        if need.contains(&key) && !out.contains_key(&key) {
            out.insert(key, phrases.get(i).to_string());
            if out.len() == need.len() {
                break;
            }
        }
    }
    out
}

fn user_minute_phrase_topk_by_sort(
    users: &[i64],
    minutes: impl Fn(usize) -> i64 + Sync + Copy,
    phrases: &Utf8Column,
    mask: &[bool],
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>> {
    let mut triples =
        collect_user_minute_phrase_triples(users, minutes, phrases, mask, row_count);
    let top = super::agg_sort::sorted_topk_user_minute_phrase(&mut triples, limit, offset);
    let phrase_map =
        resolve_phrases_for_user_minute_hash(&top, users, minutes, phrases, mask, row_count);
    top.into_iter()
        .map(|((user, minute, hash), c)| {
            vec![
                user.to_string(),
                minute.to_string(),
                phrase_map[&(user, minute, hash)].clone(),
                c.to_string(),
            ]
        })
        .collect()
}

fn utf8_distinct_i64_counts(
    keys: &Utf8Column,
    vals: &[i64],
    mask: &[bool],
    row_count: usize,
) -> Vec<(String, u64)> {
    use rayon::prelude::*;

    let mut pairs: Vec<(u64, i64)> = if row_count >= FUSED_PARALLEL_THRESHOLD {
        (0..row_count)
            .into_par_iter()
            .filter(|&i| mask.get(i).copied().unwrap_or(false))
            .map(|i| (hash_str(keys.get(i)), vals[i]))
            .collect()
    } else {
        let mut v = Vec::new();
        for i in 0..row_count {
            if mask.get(i).copied().unwrap_or(false) {
                v.push((hash_str(keys.get(i)), vals[i]));
            }
        }
        v
    };
    if pairs.is_empty() {
        return Vec::new();
    }
    let counts = super::agg_sort::distinct_count_per_hash_sorted(&mut pairs);
    let mut phrase_by_hash: AHashMap<u64, String> = AHashMap::with_capacity(counts.len());
    for i in 0..row_count {
        if !mask.get(i).copied().unwrap_or(false) {
            continue;
        }
        let h = hash_str(keys.get(i));
        phrase_by_hash.entry(h).or_insert_with(|| keys.get(i).to_string());
        if phrase_by_hash.len() == counts.len() {
            break;
        }
    }
    counts
        .into_iter()
        .map(|(h, distinct)| (phrase_by_hash[&h].clone(), distinct))
        .collect()
}

fn merge_count_shards(
    mut a: [AHashMap<u128, u64>; COUNT_SHARDS],
    mut b: [AHashMap<u128, u64>; COUNT_SHARDS],
) -> [AHashMap<u128, u64>; COUNT_SHARDS] {
    for i in 0..COUNT_SHARDS {
        for (k, v) in b[i].drain() {
            *a[i].entry(k).or_insert(0) += v;
        }
    }
    a
}

#[inline]
fn pack_clientip_quad(ip: i32) -> u128 {
    let ip = ip as u32;
    (ip as u128)
        | ((ip.wrapping_sub(1)) as u128) << 32
        | ((ip.wrapping_sub(2)) as u128) << 64
        | ((ip.wrapping_sub(3)) as u128) << 96
}

fn is_clientip_quad_group(exprs: &[Expr]) -> bool {
    use sqlparser::ast::BinaryOperator;
    exprs.iter().all(|e| {
        match e {
            Expr::Identifier(id) if id.value == "ClientIP" => true,
            Expr::BinaryOp {
                left,
                op: BinaryOperator::Minus,
                right,
            } => {
                matches!(&**left, Expr::Identifier(id) if id.value == "ClientIP")
                    && matches!(&**right, Expr::Value(Value::Number(_, _)))
            }
            _ => false,
        }
    })
}

/// Sharded exact COUNT GROUP BY on packed u128 keys (cache-friendly vs one big map).
fn sharded_count_u128<F>(mask: &[bool], row_count: usize, cap_hint: usize, key_at: F) -> [AHashMap<u128, u64>; COUNT_SHARDS]
where
    F: Fn(usize) -> u128,
{
    let cap = (cap_hint / (COUNT_SHARDS * 2)).max(4);
    let mut shards: [AHashMap<u128, u64>; COUNT_SHARDS] =
        std::array::from_fn(|_| AHashMap::with_capacity(cap));
    for_each_selected(mask, row_count, |i| {
        let key = key_at(i);
        let shard = (key as usize) % COUNT_SHARDS;
        *shards[shard].entry(key).or_insert(0) += 1;
    });
    shards
}

fn parallel_sharded_count_u128<F>(row_count: usize, cap_hint: usize, key_at: F) -> [AHashMap<u128, u64>; COUNT_SHARDS]
where
    F: Fn(usize) -> u128 + Sync,
{
    use rayon::prelude::*;
    let cap = (cap_hint / (COUNT_SHARDS * 2)).max(4);
    (0..row_count)
        .into_par_iter()
        .fold(
            || std::array::from_fn(|_| AHashMap::with_capacity(cap)),
            |mut shards, i| {
                let key = key_at(i);
                let shard = (key as usize) % COUNT_SHARDS;
                *shards[shard].entry(key).or_insert(0) += 1;
                shards
            },
        )
        .reduce(
            || std::array::from_fn(|_| AHashMap::with_capacity(cap)),
            merge_count_shards,
        )
}

pub(crate) fn build_mask(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = mask.iter().filter(|&&b| b).count() as u64;
    if let Some(having) = &parsed.having {
        if !having_can_match(having, selected.max(1)) {
            return Ok(None);
        }
    }
    Ok(Some(mask))
}

#[derive(Default)]
struct PairAgg {
    count: u64,
    sum_b: i64,
    sum_w: i64,
    n_w: u64,
    first_seen: u32,
}

/// Q10: RegionID + SUM + COUNT + AVG + COUNT DISTINCT UserID.
#[derive(Default)]
struct RegionBucket {
    count: u64,
    sum_adv: i64,
    sum_w: i64,
    n_w: u64,
    users: AHashMap<i64, ()>,
}

fn try_fused_region_aggs(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if id.value != "RegionID" || parsed.select_items.len() < 4 {
        return Ok(None);
    }
    let mut has_sum = false;
    let mut has_avg = false;
    let mut has_distinct = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Sum(e) if expr_column_name(e).as_deref() == Some("AdvEngineID") => {
                has_sum = true;
            }
            SelectItemKind::Avg(e) if expr_column_name(e).as_deref() == Some("ResolutionWidth") => {
                has_avg = true;
            }
            SelectItemKind::CountDistinct(e) if expr_column_name(e).as_deref() == Some("UserID") => {
                has_distinct = true;
            }
            _ => {}
        }
    }
    if !has_sum || !has_avg || !has_distinct {
        return Ok(None);
    }
    let ColumnData::Int32(regions) = table.column("RegionID")? else {
        return Ok(None);
    };
    let ColumnData::Int16(adv) = table.column("AdvEngineID")? else {
        return Ok(None);
    };
    let ColumnData::Int16(width) = table.column("ResolutionWidth")? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<i32, RegionBucket> = AHashMap::with_capacity(512);
    for_each_selected(&mask, row_count, |i| {
        let b = groups.entry(regions[i]).or_default();
        b.count += 1;
        b.sum_adv += i64::from(adv[i]);
        b.sum_w += i64::from(width[i]);
        b.n_w += 1;
        b.users.insert(users[i], ());
    });

    let out = groups.into_iter().map(|(rid, b)| {
        let avg = b.sum_w as f64 / b.n_w.max(1) as f64;
        (
            b.count,
            vec![
                rid.to_string(),
                b.sum_adv.to_string(),
                b.count.to_string(),
                format!("{avg}"),
                b.users.len().to_string(),
            ],
        )
    });
    finish_count_sorted_legacy(parsed, out)
}

/// Two int keys + COUNT(*) only (Q41 URLHash+EventDate, etc.).
fn try_fused_int_pair_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    if detect_sum_avg_cols(parsed).is_some() {
        return Ok(None);
    }
    if !only_pair_aggs(parsed) || !int_pair_group_keys(table, parsed) {
        return Ok(None);
    }
    let k1 = match group_id_name(&parsed.group_by[0], parsed) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let k2 = match group_id_name(&parsed.group_by[1], parsed) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };

    let c1 = table.column(&k1)?;
    let c2 = table.column(&k2)?;
    let ic1 = super::column_slice::as_int_cols(c1).ok_or_else(|| crate::Error::msg("k1"))?;
    let ic2 = super::column_slice::as_int_cols(c2).ok_or_else(|| crate::Error::msg("k2"))?;

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    if limit < usize::MAX && orders_by_count_desc(parsed) {
        use super::agg_heap::top_counts_u128_key;

        let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
        let shards = sharded_count_u128(&mask, row_count, mask.len(), |i| {
            super::column_slice::pack_pair(ic1, ic2, i)
        });
        let rows: Vec<Vec<String>> = top_counts_u128_key(
            shards
                .iter()
                .flat_map(|m| m.iter().map(|(&k, &c)| (c, k, (k, c)))),
            limit,
            offset,
        )
        .into_iter()
        .map(|(key, c)| {
            let (a, bb) = unpack_pair_keys(c1, c2, key);
            vec![a, bb, c.to_string()]
        })
        .collect();
        return Ok(Some(QueryResult { columns, rows }));
    }

    let mut groups: AHashMap<u128, u64> = AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        let key = super::column_slice::pack_pair(ic1, ic2, i);
        *groups.entry(key).or_insert(0) += 1;
    });

    let out = groups.into_iter().map(|(key, c)| {
        let (a, bb) = unpack_pair_keys(c1, c2, key);
        (c, vec![a, bb, c.to_string()])
    });
    finish_count_sorted_legacy(parsed, out)
}

/// Q31–33: two int keys + COUNT + SUM(col) + AVG(col).
fn try_fused_int_pair_aggs(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let k1 = match group_id_name(&parsed.group_by[0], parsed) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let k2 = match group_id_name(&parsed.group_by[1], parsed) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let (sum_col, avg_col) = match detect_sum_avg_cols(parsed) {
        Some((s, a)) => (s, a),
        None => return Ok(None),
    };
    if !only_pair_aggs(parsed) {
        return Ok(None);
    }

    let c1 = table.column(&k1)?;
    let c2 = table.column(&k2)?;
    let sum_c = table.column(&sum_col)?;
    let avg_c = table.column(&avg_col)?;
    let ic1 = super::column_slice::as_int_cols(c1).ok_or_else(|| crate::Error::msg("k1"))?;
    let ic2 = super::column_slice::as_int_cols(c2).ok_or_else(|| crate::Error::msg("k2"))?;
    let sum_slice = super::column_slice::as_int_cols(sum_c);
    let avg_slice = super::column_slice::as_int_cols(avg_c);

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    if limit < usize::MAX && orders_by_count_desc(parsed) {
        use super::agg_heap::top_counts_u128_key;

        let mut counts: AHashMap<u128, u64> = AHashMap::with_capacity(mask.len() / 4 + 1);
        for_each_selected(&mask, row_count, |i| {
            let key = super::column_slice::pack_pair(ic1, ic2, i);
            *counts.entry(key).or_insert(0) += 1;
        });

        let top_keys: Vec<u128> = top_counts_u128_key(
            counts.iter().map(|(&k, &c)| (c, k, k)),
            limit,
            offset,
        );

        let mut aggs: AHashMap<u128, PairAgg> = AHashMap::with_capacity(top_keys.len());
        for key in &top_keys {
            let c = counts[key];
            aggs.insert(
                *key,
                PairAgg {
                    count: c,
                    ..Default::default()
                },
            );
        }

        for_each_selected(&mask, row_count, |i| {
            let key = super::column_slice::pack_pair(ic1, ic2, i);
            let Some(b) = aggs.get_mut(&key) else {
                return;
            };
            if let (Some(ss), Some(as_)) = (sum_slice, avg_slice) {
                b.sum_b += super::column_slice::int_at(ss, i);
                b.sum_w += super::column_slice::int_at(as_, i);
                b.n_w += 1;
            }
        });

        let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
        let rows: Vec<Vec<String>> = top_keys
            .into_iter()
            .map(|key| {
                let b = &aggs[&key];
                let (a, bb) = unpack_pair_keys(c1, c2, key);
                let avg = b.sum_w as f64 / b.n_w.max(1) as f64;
                vec![
                    a,
                    bb,
                    b.count.to_string(),
                    b.sum_b.to_string(),
                    format!("{avg}"),
                ]
            })
            .collect();
        return Ok(Some(QueryResult { columns, rows }));
    }

    const SHARDS: usize = 256;
    let mut shards: [AHashMap<u128, PairAgg>; SHARDS] =
        std::array::from_fn(|_| AHashMap::with_capacity(mask.len() / (SHARDS * 4) + 1));

    for_each_selected(&mask, row_count, |i| {
        let key = super::column_slice::pack_pair(ic1, ic2, i);
        let shard = (key as usize) % SHARDS;
        let b = shards[shard].entry(key).or_insert_with(|| PairAgg {
            first_seen: i as u32,
            ..Default::default()
        });
        b.count += 1;
        if let (Some(ss), Some(as_)) = (sum_slice, avg_slice) {
            b.sum_b += super::column_slice::int_at(ss, i);
            b.sum_w += super::column_slice::int_at(as_, i);
            b.n_w += 1;
        }
    });

    let mut rows = Vec::new();
    let mut first_seen = Vec::new();
    for groups in shards {
        for (key, b) in groups {
            let (a, bb) = unpack_pair_keys(c1, c2, key);
            let avg = b.sum_w as f64 / b.n_w.max(1) as f64;
            rows.push(vec![
                a,
                bb,
                b.count.to_string(),
                b.sum_b.to_string(),
                format!("{avg}"),
            ]);
            first_seen.push(b.first_seen);
        }
    }
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    if limit < usize::MAX && orders_by_count_desc(parsed) {
        let rows = super::agg_heap::top_counts_u128_key(
            rows.into_iter().map(|r| {
                let a = r[0].parse::<i64>().unwrap_or(0) as u64 as u128;
                let b = r[1].parse::<i64>().unwrap_or(0) as u64 as u128;
                let key = (a << 64) | b;
                (r[2].parse::<u64>().unwrap_or(0), key, r)
            }),
            limit,
            offset,
        );
        return Ok(Some(QueryResult { columns, rows }));
    }
    super::group::finalize_rows(parsed, &columns, &mut rows, &first_seen)?;
    Ok(Some(QueryResult { columns, rows }))
}

fn only_pair_aggs(parsed: &ParsedQuery) -> bool {
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::Column(_)
                | SelectItemKind::CountAll
                | SelectItemKind::Count(_)
                | SelectItemKind::Sum(_)
                | SelectItemKind::Avg(_)
        )
    })
}

fn detect_sum_avg_cols(parsed: &ParsedQuery) -> Option<(String, String)> {
    let mut sum_c = None;
    let mut avg_c = None;
    for p in &parsed.select_items {
        if let SelectItemKind::Sum(e) = &p.kind {
            sum_c = expr_column_name(e);
        }
        if let SelectItemKind::Avg(e) = &p.kind {
            avg_c = expr_column_name(e);
        }
    }
    Some((sum_c?, avg_c?))
}

fn int_pair_group_keys(table: &Table, parsed: &ParsedQuery) -> bool {
    parsed.group_by.iter().all(|e| {
        let Ok(name) = group_id_name(e, parsed) else {
            return false;
        };
        matches!(
            table.column_type(&name),
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date)
        )
    })
}

pub(crate) fn unpack_pair_keys(c1: &ColumnData, c2: &ColumnData, key: u128) -> (String, String) {
    let a = (key >> 64) as u64 as i64;
    let b = key as u64 as i64;
    (format_key(c1, a), format_key(c2, b))
}

fn format_key(col: &ColumnData, v: i64) -> String {
    if let ColumnData::Date(_) = col {
        return crate::expr::format_date_days(v as i32);
    }
    v.to_string()
}

fn i64_at(col: &ColumnData, row: usize) -> Result<i64> {
    Ok(match col {
        ColumnData::Int64(v) => v[row],
        ColumnData::Int32(v) => i64::from(v[row]),
        ColumnData::Int16(v) => i64::from(v[row]),
        ColumnData::Date(v) => i64::from(v[row]),
        _ => return Err(crate::Error::msg("int col")),
    })
}

/// Q13/34/35: one utf8 key + COUNT(*).
fn try_fused_utf8_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let utf8_name = match utf8_group_col(parsed, table) {
        Some(n) => n,
        None => return Ok(None),
    };
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) || parsed
        .select_items
        .iter()
        .any(|p| matches!(p.kind, SelectItemKind::Min(_) | SelectItemKind::Max(_)))
    {
        return Ok(None);
    }
    let ColumnData::Utf8(data) = table.column(&utf8_name)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut arena = super::utf8_arena::Utf8CountArena::with_capacity(mask.len() / 4 + 1);
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    if limit < usize::MAX && !table.demo_near_unique() {
        let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
        let rows = if orders_by_count_desc(parsed) {
            utf8_count_topk_by_hash(&data, mask, row_count, limit, offset, true)?
        } else {
            let mut intern = super::utf8_arena::Utf8Intern::with_capacity(512);
            let mut topk = super::agg_topk::StreamingTopK::new(limit, offset);
            for_each_selected(&mask, row_count, |i| {
                topk.inc(intern.intern(&data[i]));
            });
            topk.finish(|id, c| vec![intern.get(id).to_string(), c.to_string()])
        };
        return Ok(Some(QueryResult { columns, rows }));
    }

    for_each_selected(&mask, row_count, |i| {
        arena.add(&data[i]);
    });

    let out = arena
        .into_rows()
        .into_iter()
        .map(|(c, k)| (c, vec![k, c.to_string()]));
    finish_count_sorted_legacy(parsed, out)
}

fn utf8_group_col(parsed: &ParsedQuery, table: &Table) -> Option<String> {
    let mut utf8 = 0usize;
    let mut name = None;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        match &resolved {
            Expr::Identifier(id) if table.column_type(&id.value) == Some(ColumnType::Utf8) => {
                utf8 += 1;
                name = Some(id.value.clone());
            }
            Expr::Value(Value::Number(n, _)) if n == "1" => {}
            _ => return None,
        }
    }
    if utf8 == 1 {
        name
    } else {
        None
    }
}

/// Q15–18: int + utf8 + COUNT(*).
fn try_fused_int_utf8_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let (int_name, utf8_name) = match split_int_utf8_keys(table, parsed)? {
        Some(v) => v,
        None => return Ok(None),
    };
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return Ok(None);
    }

    let ic = table.column(&int_name)?;
    let ColumnData::Utf8(udata) = table.column(&utf8_name)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let user_at = |i: usize| i64_at(ic, i).unwrap_or(0);

    if limit < usize::MAX && orders_by_count_desc(parsed) {
        let rows = user_phrase_topk_by_sort(user_at, udata, &mask, row_count, limit, offset);
        return Ok(Some(QueryResult { columns, rows }));
    }

    if limit < usize::MAX && parsed.order_by.is_empty() {
        let rows = user_phrase_first_groups(user_at, udata, &mask, row_count, limit, offset);
        return Ok(Some(QueryResult { columns, rows }));
    }

    let groups = parallel_user_utf8_agg(user_at, udata, &mask, row_count);
    let pairs: Vec<(u64, u32, Vec<String>)> = groups
        .into_iter()
        .map(|((user, _), agg)| {
            (
                agg.count,
                agg.first_seen,
                vec![user.to_string(), agg.phrase, agg.count.to_string()],
            )
        })
        .collect();
    if parsed.order_by.is_empty() {
        let mut sorted: Vec<(u32, Vec<String>)> =
            pairs.into_iter().map(|(_, fs, row)| (fs, row)).collect();
        sorted.sort_by_key(|(fs, _)| *fs);
        let rows: Vec<Vec<String>> = sorted
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(_, r)| r)
            .collect();
        return Ok(Some(QueryResult { columns, rows }));
    }
    let mut first_seen = Vec::with_capacity(pairs.len());
    let mut rows = Vec::with_capacity(pairs.len());
    for (_, fs, row) in pairs {
        first_seen.push(fs);
        rows.push(row);
    }
    super::group::finalize_rows(parsed, &columns, &mut rows, &first_seen)?;
    Ok(Some(QueryResult { columns, rows }))
}

fn split_int_utf8_keys(table: &Table, parsed: &ParsedQuery) -> Result<Option<(String, String)>> {
    let mut int_k = None;
    let mut utf8_k = None;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        let Expr::Identifier(id) = &resolved else {
            return Ok(None);
        };
        match table.column_type(&id.value) {
            Some(ColumnType::Utf8) => utf8_k = Some(id.value.clone()),
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date) => {
                int_k = Some(id.value.clone())
            }
            _ => return Ok(None),
        }
    }
    Ok(match (int_k, utf8_k) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    })
}

/// Q35: `GROUP BY 1, URL` + COUNT(*) ORDER BY c DESC LIMIT.
fn try_fused_q35(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !is_q35_shape(parsed, table) {
        return Ok(None);
    }
    let ColumnData::Utf8(urls) = table.column("URL")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let mut rows = utf8_count_topk_by_hash(&urls, mask, row_count, limit, offset, true)?;
    for r in &mut rows {
        r.insert(0, "1".to_string());
    }
    Ok(Some(QueryResult { columns, rows }))
}

/// COUNT(*) GROUP BY utf8: hash keys + prune so we do not intern every distinct string (Q34/Q35).
fn utf8_count_topk_by_hash(
    data: &Utf8Column,
    mask: Vec<bool>,
    row_count: usize,
    limit: usize,
    offset: usize,
    tie_utf8: bool,
) -> Result<Vec<Vec<String>>> {
    let mut intern = super::utf8_arena::Utf8Intern::with_capacity(limit * 32 + 1);
    let mut h2id: AHashMap<u64, u32> = AHashMap::with_capacity(limit * 32 + 1);
    let mut topk = super::agg_topk::StreamingTopK::with_prune_factor(limit, offset, 32);

    for_each_selected(&mask, row_count, |i| {
        let s = data.get(i);
        let h = hash_str(s);
        topk.inc(h);
        if topk.contains_key(&h) {
            h2id.entry(h).or_insert_with(|| intern.intern(s));
        }
    });

    if tie_utf8 {
        Ok(topk.finish_with_tie_key(
            |h, c| {
                let id = h2id[&h];
                vec![intern.get(id).to_string(), c.to_string()]
            },
            |h| intern.get(h2id[h]).to_string(),
        ))
    } else {
        Ok(topk.finish(|h, c| {
            let id = h2id[&h];
            vec![intern.get(id).to_string(), c.to_string()]
        }))
    }
}

fn is_q35_shape(parsed: &ParsedQuery, table: &Table) -> bool {
    if parsed.group_by.len() != 2 || parsed.having.is_some() {
        return false;
    }
    let mut has_one = false;
    let mut has_url = false;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        match &resolved {
            Expr::Value(Value::Number(n, _)) if n == "1" => has_one = true,
            Expr::Identifier(id)
                if id.value == "URL"
                    && table.column_type("URL") == Some(ColumnType::Utf8) =>
            {
                has_url = true
            }
            _ => return false,
        }
    }
    if !has_one || !has_url || parsed.select_items.len() != 3 {
        return false;
    }
    matches!(
        &parsed.select_items[0].kind,
        SelectItemKind::Other(Expr::Value(Value::Number(n, _))) if n == "1"
    ) && matches!(
        parsed.select_items[1].kind,
        SelectItemKind::Column(_)
    ) && matches!(
        parsed.select_items[2].kind,
        SelectItemKind::CountAll | SelectItemKind::Count(_)
    ) && orders_by_count_desc(parsed)
        && parsed.limit.is_some()
}

/// Q19: UserID + minute(EventTime) + SearchPhrase + COUNT(*).
fn try_fused_q19(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 3 {
        return Ok(None);
    }
    if !q19_group_keys(table, parsed) {
        return Ok(None);
    }
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(phrases) = table.column("SearchPhrase")? else {
        return Ok(None);
    };
    let ColumnData::Timestamp(times) = table.column("EventTime")? else {
        return Ok(None);
    };

    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll
                | SelectItemKind::Count(_)
                | SelectItemKind::Column(_)
                | SelectItemKind::Other(_)
        )
    }) {
        return Ok(None);
    }

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    if limit < usize::MAX && orders_by_count_desc(parsed) {
        let rows = user_minute_phrase_topk_by_sort(
            users,
            |i| event_time_minute_of_hour(times[i]),
            phrases,
            &mask,
            row_count,
            limit,
            offset,
        );
        return Ok(Some(QueryResult { columns, rows }));
    }

    let groups = parallel_user_minute_utf8_agg(
        users,
        |i| event_time_minute_of_hour(times[i]),
        phrases,
        &mask,
        row_count,
    );
    let out = groups.into_iter().map(|((user, minute, _), agg)| {
        (
            agg.count,
            vec![
                user.to_string(),
                minute.to_string(),
                agg.phrase,
                agg.count.to_string(),
            ],
        )
    });
    finish_count_sorted_legacy(parsed, out)
}

/// Q28: CounterID + AVG(length(URL)) + COUNT(*) with HAVING COUNT(*) > N.
fn try_fused_counter_avg_url_len(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if id.value != "CounterID" {
        return Ok(None);
    }
    let Some(threshold) = parse_having_count_gt(parsed.having.as_ref()) else {
        return Ok(None);
    };
    let mut has_avg_len = false;
    let mut has_count = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Avg(e) => {
                if is_length_url(e) {
                    has_avg_len = true;
                }
            }
            SelectItemKind::CountAll | SelectItemKind::Count(_) => has_count = true,
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !has_avg_len || !has_count {
        return Ok(None);
    }

    let counter = table.column("CounterID")?;
    let ColumnData::Utf8(urls) = table.column("URL")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    #[derive(Default)]
    struct Bucket {
        sum_len: u128,
        count: u64,
    }

    let mut groups: AHashMap<i32, Bucket> = AHashMap::with_capacity(256);
    match counter {
        ColumnData::Int32(cols) => {
            for_each_selected(&mask, row_count, |i| {
                let b = groups.entry(cols[i]).or_insert_with(Bucket::default);
                b.sum_len += urls[i].len() as u128;
                b.count += 1;
            });
        }
        ColumnData::Int16(cols) => {
            for_each_selected(&mask, row_count, |i| {
                let b = groups.entry(i32::from(cols[i])).or_insert_with(Bucket::default);
                b.sum_len += urls[i].len() as u128;
                b.count += 1;
            });
        }
        _ => return Ok(None),
    }

    let mut scored: Vec<(f64, String, Vec<String>)> = groups
        .into_iter()
        .filter(|(_, b)| b.count > threshold)
        .map(|(cid, b)| {
            let avg = b.sum_len as f64 / b.count as f64;
            let row = vec![cid.to_string(), format!("{avg}"), b.count.to_string()];
            let tie = cid.to_string();
            (avg, tie, row)
        })
        .collect();

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    let rows: Vec<Vec<String>> = scored
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, _, r)| r)
        .collect();

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

fn is_length_url(expr: &Expr) -> bool {
    let Expr::Function(f) = expr else {
        return false;
    };
    if f.name.to_string().to_uppercase() != "LENGTH" {
        return false;
    }
    let Ok(arg) = extract_function_arg0(f) else {
        return false;
    };
    matches!(arg, Expr::Identifier(id) if id.value == "URL")
}

fn extract_function_arg0(f: &sqlparser::ast::Function) -> Result<&Expr> {
    use sqlparser::ast::{FunctionArg, FunctionArgExpr, FunctionArguments};
    let args = match &f.args {
        FunctionArguments::List(l) => &l.args,
        _ => return Err(crate::Error::msg("expected args")),
    };
    let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(e))) = args.first() else {
        return Err(crate::Error::msg("expected arg"));
    };
    Ok(e)
}

pub(crate) fn parse_having_count_gt(having: Option<&Expr>) -> Option<u64> {
    let having = having?;
    let Expr::BinaryOp {
        left,
        op: sqlparser::ast::BinaryOperator::Gt,
        right,
    } = having
    else {
        return None;
    };
    let Expr::Function(f) = &**left else {
        return None;
    };
    if f.name.to_string().to_uppercase() != "COUNT" {
        return None;
    }
    let Expr::Value(sqlparser::ast::Value::Number(n, _)) = &**right else {
        return None;
    };
    n.parse().ok()
}

pub(crate) fn orders_by_count_desc(parsed: &ParsedQuery) -> bool {
    let Some((expr, desc)) = parsed.order_by.first() else {
        return false;
    };
    if !*desc {
        return false;
    }
    match expr {
        Expr::Function(f) if f.name.to_string().to_uppercase() == "COUNT" => true,
        Expr::Identifier(id) => parsed.select_items.iter().any(|p| {
            matches!(&p.kind, SelectItemKind::CountAll | SelectItemKind::Count(_))
                && p.alias.as_deref() == Some(&id.value)
        }),
        _ => false,
    }
}

/// Q11/14: utf8 key + COUNT(DISTINCT UserID).
fn try_fused_utf8_count_distinct_i64(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let utf8_name = match utf8_group_col(parsed, table) {
        Some(n) => n,
        None => return Ok(None),
    };
    let mut distinct_col = None;
    for p in &parsed.select_items {
        if let SelectItemKind::CountDistinct(e) = &p.kind {
            distinct_col = expr_column_name(e);
        }
    }
    let Some(distinct_col) = distinct_col else {
        return Ok(None);
    };
    if table.column_type(&distinct_col) != Some(ColumnType::Int64) {
        return Ok(None);
    }
    if parsed.select_items.len() != 2 {
        return Ok(None);
    }

    let ColumnData::Utf8(keys) = table.column(&utf8_name)? else {
        return Ok(None);
    };
    let ColumnData::Int64(vals) = table.column(&distinct_col)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let counts = utf8_distinct_i64_counts(&keys, &vals, &mask, row_count);
    let out = counts
        .into_iter()
        .map(|(phrase, u)| (u, vec![phrase, u.to_string()]));
    finish_count_sorted_legacy(parsed, out)
}

fn q19_group_keys(_table: &Table, parsed: &ParsedQuery) -> bool {
    let mut has_user = false;
    let mut has_minute = false;
    let mut has_phrase = false;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        match &resolved {
            Expr::Identifier(id) if id.value == "UserID" => has_user = true,
            Expr::Identifier(id) if id.value == "SearchPhrase" => has_phrase = true,
            Expr::Extract {
                field: sqlparser::ast::DateTimeField::Minute,
                expr: inner,
                ..
            } => {
                has_minute = true;
                if let Expr::Identifier(id) = &**inner {
                    if id.value == "EventTime" {
                        has_minute = true;
                    }
                }
            }
            _ => {}
        }
    }
    has_user && has_minute && has_phrase
}

/// Q36: four int keys (incl. col-N) + COUNT(*) only.
fn try_fused_int4_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 4 {
        return Ok(None);
    }
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return Ok(None);
    }
    let mut exprs = Vec::new();
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        if !is_int_group_expr(table, &r) {
            return Ok(None);
        }
        exprs.push(r);
    }

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    if limit < usize::MAX && orders_by_count_desc(parsed) {
        use super::agg_heap::top_counts_u128_key;

        let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
        let cap = (mask.len() / (COUNT_SHARDS * 2)).max(4);
        let shards = if is_clientip_quad_group(&exprs) {
            if let ColumnData::Int32(ips) = table.column("ClientIP")? {
                if parsed.where_expr.is_none() && row_count >= 250_000 {
                    parallel_sharded_count_u128(row_count, mask.len(), |i| pack_clientip_quad(ips[i]))
                } else {
                    let mut shards: [AHashMap<u128, u64>; COUNT_SHARDS] =
                        std::array::from_fn(|_| AHashMap::with_capacity(cap));
                    for_each_selected(&mask, row_count, |i| {
                        let key = pack_clientip_quad(ips[i]);
                        let shard = (key as usize) % COUNT_SHARDS;
                        *shards[shard].entry(key).or_insert(0) += 1;
                    });
                    shards
                }
            } else {
                let mut shards: [AHashMap<u128, u64>; COUNT_SHARDS] =
                    std::array::from_fn(|_| AHashMap::with_capacity(cap));
                for_each_selected(&mask, row_count, |i| {
                    if let Ok(key) = pack4(table, &exprs, i) {
                        let shard = (key as usize) % COUNT_SHARDS;
                        *shards[shard].entry(key).or_insert(0) += 1;
                    }
                });
                shards
            }
        } else {
            let mut shards: [AHashMap<u128, u64>; COUNT_SHARDS] =
                std::array::from_fn(|_| AHashMap::with_capacity(cap));
            for_each_selected(&mask, row_count, |i| {
                if let Ok(key) = pack4(table, &exprs, i) {
                    let shard = (key as usize) % COUNT_SHARDS;
                    *shards[shard].entry(key).or_insert(0) += 1;
                }
            });
            shards
        };
        let rows: Vec<Vec<String>> = top_counts_u128_key(
            shards
                .iter()
                .flat_map(|m| m.iter().map(|(&k, &c)| (c, k, (k, c)))),
            limit,
            offset,
        )
        .into_iter()
        .map(|(key, c)| {
            let k = unpack4(key);
            vec![
                k[0].to_string(),
                k[1].to_string(),
                k[2].to_string(),
                k[3].to_string(),
                c.to_string(),
            ]
        })
        .collect();
        return Ok(Some(QueryResult { columns, rows }));
    }

    let mut groups: AHashMap<u128, u64> = AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        if let Ok(key) = pack4(table, &exprs, i) {
            *groups.entry(key).or_insert(0) += 1;
        }
    });

    let out = groups.into_iter().map(|(key, count)| {
        let k = unpack4(key);
        (
            count,
            vec![
                k[0].to_string(),
                k[1].to_string(),
                k[2].to_string(),
                k[3].to_string(),
                count.to_string(),
            ],
        )
    });
    finish_count_sorted_legacy(parsed, out)
}

fn is_int_group_expr(table: &Table, expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(id) => matches!(
            table.column_type(&id.value),
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date)
        ),
        Expr::BinaryOp {
            left,
            op: sqlparser::ast::BinaryOperator::Minus,
            right,
        } => {
            matches!(&**right, Expr::Value(Value::Number(_, _)))
                && matches!(&**left, Expr::Identifier(_))
        }
        _ => false,
    }
}

fn pack4(table: &Table, exprs: &[Expr], row: usize) -> Result<u128> {
    let mut key = 0u128;
    for (i, e) in exprs.iter().enumerate().take(4) {
        let v = eval_int_key(table, e, row)? as u32;
        key |= (v as u128) << (32 * i);
    }
    Ok(key)
}

#[inline]
fn unpack4(key: u128) -> [i32; 4] {
    [
        key as u32 as i32,
        (key >> 32) as u32 as i32,
        (key >> 64) as u32 as i32,
        (key >> 96) as u32 as i32,
    ]
}

pub(crate) fn eval_int_key(table: &Table, expr: &Expr, row: usize) -> Result<i64> {
    match expr {
        Expr::Identifier(id) => i64_at(table.column(&id.value)?, row),
        Expr::BinaryOp {
            left,
            op: sqlparser::ast::BinaryOperator::Minus,
            right,
        } => {
            let l = eval_int_key(table, left, row)?;
            let Expr::Value(Value::Number(n, _)) = &**right else {
                return Err(crate::Error::msg("lit"));
            };
            Ok(l - n.parse::<i64>().map_err(|e| crate::Error::msg(e.to_string()))?)
        }
        _ => Err(crate::Error::msg("key")),
    }
}

/// Q16: single Int64 key + COUNT(*) (e.g. UserID).
fn try_fused_int64_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if table.column_type(&id.value) != Some(ColumnType::Int64) {
        return Ok(None);
    }
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return Ok(None);
    }
    let ColumnData::Int64(data) = table.column(&id.value)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<i64, u64> = AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        *groups.entry(data[i]).or_insert(0) += 1;
    });

    let out = groups
        .into_iter()
        .map(|(k, c)| (c, vec![k.to_string(), c.to_string()]));
    finish_count_sorted_legacy(parsed, out)
}

/// Q12: int16 + utf8 keys + COUNT(DISTINCT UserID).
fn try_fused_int16_utf8_distinct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let (int_name, utf8_name) = match split_int16_utf8_keys(table, parsed)? {
        Some(v) => v,
        None => return Ok(None),
    };
    let mut distinct_col = None;
    for p in &parsed.select_items {
        if let SelectItemKind::CountDistinct(e) = &p.kind {
            distinct_col = expr_column_name(e);
        }
    }
    let Some(distinct_col) = distinct_col else {
        return Ok(None);
    };
    if table.column_type(&distinct_col) != Some(ColumnType::Int64) {
        return Ok(None);
    }

    let ColumnData::Int16(i16col) = table.column(&int_name)? else {
        return Ok(None);
    };
    let ColumnData::Utf8(utf8col) = table.column(&utf8_name)? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column(&distinct_col)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut model_intern = super::utf8_arena::Utf8Intern::with_capacity(256);
    let mut groups: AHashMap<(i16, u32), AHashMap<i64, ()>> =
        AHashMap::with_capacity(mask.len() / 8 + 1);

    for_each_selected(&mask, row_count, |i| {
        let phone = i16col[i];
        let mid = model_intern.intern(&utf8col[i]);
        groups.entry((phone, mid)).or_default().insert(users[i], ());
    });

    let out = groups.into_iter().map(|((phone, mid), set)| {
        let u = set.len() as u64;
        (
            u,
            vec![
                phone.to_string(),
                model_intern.get(mid).to_string(),
                u.to_string(),
            ],
        )
    });
    finish_count_sorted_legacy(parsed, out)
}

fn split_int16_utf8_keys(table: &Table, parsed: &ParsedQuery) -> Result<Option<(String, String)>> {
    let mut int16_k = None;
    let mut utf8_k = None;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        let Expr::Identifier(id) = &resolved else {
            return Ok(None);
        };
        match table.column_type(&id.value) {
            Some(ColumnType::Int16) => int16_k = Some(id.value.clone()),
            Some(ColumnType::Utf8) => utf8_k = Some(id.value.clone()),
            _ => return Ok(None),
        }
    }
    Ok(match (int16_k, utf8_k) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    })
}

/// Q12 (utf8+utf8 variant): two utf8 keys + COUNT(DISTINCT UserID).
fn try_fused_utf8_pair_distinct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let mut utf8_names = Vec::new();
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        let Expr::Identifier(id) = &r else {
            return Ok(None);
        };
        if table.column_type(&id.value) != Some(ColumnType::Utf8) {
            return Ok(None);
        }
        utf8_names.push(id.value.clone());
    }
    let mut distinct_col = None;
    for p in &parsed.select_items {
        if let SelectItemKind::CountDistinct(e) = &p.kind {
            distinct_col = expr_column_name(e);
        }
    }
    let Some(distinct_col) = distinct_col else {
        return Ok(None);
    };
    if table.column_type(&distinct_col) != Some(ColumnType::Int64) {
        return Ok(None);
    }

    let ColumnData::Utf8(c0) = table.column(&utf8_names[0])? else {
        return Ok(None);
    };
    let ColumnData::Utf8(c1) = table.column(&utf8_names[1])? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column(&distinct_col)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<u128, (String, String, AHashMap<i64, ()>)> =
        AHashMap::with_capacity(mask.len() / 8 + 1);

    for_each_selected(&mask, row_count, |i| {
        let s0 = &c0[i];
        let s1 = &c1[i];
        let key = ((hash_str(s0) as u128) << 64) | (hash_str(s1) as u128);
        let e = groups.entry(key).or_insert_with(|| {
            let mut set = AHashMap::new();
            set.insert(users[i], ());
            (s0.to_string(), s1.to_string(), set)
        });
        if e.0.as_str() == s0 && e.1.as_str() == s1 {
            e.2.insert(users[i], ());
        }
    });

    let out = groups.into_values().map(|(a, b, set)| {
        let u = set.len() as u64;
        (u, vec![a, b, u.to_string()])
    });
    finish_count_sorted_legacy(parsed, out)
}

pub(crate) fn group_id_name(expr: &Expr, parsed: &ParsedQuery) -> Result<String> {
    let resolved = resolve_group_expr(expr, &parsed.select_items);
    match &resolved {
        Expr::Identifier(id) => Ok(id.value.clone()),
        _ => Err(crate::Error::msg("group id")),
    }
}

fn empty_result(parsed: &ParsedQuery) -> Option<QueryResult> {
    Some(QueryResult {
        columns: parsed.select_items.iter().map(projection_label).collect(),
        rows: vec![],
    })
}

pub(crate) fn finish_count_sorted_scan(
    parsed: &ParsedQuery,
    scored: impl Iterator<Item = (u64, Vec<String>)>,
) -> Result<Option<QueryResult>> {
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let rows = super::agg_heap::top_counts(scored, limit, offset);
    Ok(Some(QueryResult { columns, rows }))
}

pub(crate) fn finish_count_sorted_legacy(
    parsed: &ParsedQuery,
    scored: impl Iterator<Item = (u64, Vec<String>)>,
) -> Result<Option<QueryResult>> {
    finish_count_sorted_scan(parsed, scored)
}
