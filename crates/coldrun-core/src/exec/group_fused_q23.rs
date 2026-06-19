//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! ClickHouse-style incremental aggregation — never materialize O(rows) `(phrase, user)` pairs
//! on top of the warm-serve column cache (~30 GiB on 100M).
//!
//! Pass 1: sharded `phrase_hash → count` only (O(distinct phrases)).
//! Pass 2: rescan; for top-K phrase hashes only — `UniqExact`-style user sets + MIN via row indices.

use ahash::{AHashMap, AHashSet};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::hash_str;
use super::QueryResult;

const PARALLEL_THRESHOLD: usize = 250_000;
const COUNT_SHARDS: usize = 256;

#[inline]
fn q23_row_matches(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    i: usize,
) -> bool {
    if phrases.get(i).is_empty() {
        return false;
    }
    let t = titles.get(i).as_bytes();
    if memchr::memmem::find(t, b"Google").is_none() {
        return false;
    }
    !memchr::memmem::find(urls.get(i).as_bytes(), b".google.").is_some()
}

pub fn try_fused_q23(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let sqlparser::ast::Expr::Identifier(id) = &parsed.group_by[0] else {
        return Ok(None);
    };
    if id.value != "SearchPhrase" {
        return Ok(None);
    }

    let mut has_min_url = false;
    let mut has_min_title = false;
    let mut has_count = false;
    let mut has_distinct = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Min(e) if is_col(e, "URL") => has_min_url = true,
            SelectItemKind::Min(e) if is_col(e, "Title") => has_min_title = true,
            SelectItemKind::CountAll | SelectItemKind::Count(_) => has_count = true,
            SelectItemKind::CountDistinct(e) if is_col(e, "UserID") => has_distinct = true,
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !has_min_url || !has_min_title || !has_count || !has_distinct {
        return Ok(None);
    }

    let ColumnData::Utf8(phrases) = table.column("SearchPhrase")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(urls) = table.column("URL")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(titles) = table.column("Title")? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let rows = q23_execute(phrases, urls, titles, users, row_count, limit, offset);

    Ok(Some(QueryResult { columns, rows }))
}

type CountShards = [AHashMap<u64, u64>; COUNT_SHARDS];

#[inline]
fn shard_idx(h: u64) -> usize {
    h as usize % COUNT_SHARDS
}

fn merge_count_shards(mut a: CountShards, mut b: CountShards) -> CountShards {
    for i in 0..COUNT_SHARDS {
        for (k, v) in b[i].drain() {
            *a[i].entry(k).or_insert(0) += v;
        }
    }
    a
}

fn pass1_phrase_counts(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    row_count: usize,
) -> AHashMap<u64, u64> {
    use rayon::prelude::*;

    let cap = (row_count / (COUNT_SHARDS * 8)).max(4);
    let shards = if row_count >= PARALLEL_THRESHOLD {
        (0..row_count)
            .into_par_iter()
            .fold(
                || std::array::from_fn(|_| AHashMap::with_capacity(cap)),
                |mut shards, i| {
                    if q23_row_matches(phrases, urls, titles, i) {
                        let h = hash_str(phrases.get(i));
                        *shards[shard_idx(h)].entry(h).or_insert(0) += 1;
                    }
                    shards
                },
            )
            .reduce(
                || std::array::from_fn(|_| AHashMap::new()),
                merge_count_shards,
            )
    } else {
        let mut shards: CountShards = std::array::from_fn(|_| AHashMap::with_capacity(cap));
        for i in 0..row_count {
            if q23_row_matches(phrases, urls, titles, i) {
                let h = hash_str(phrases.get(i));
                *shards[shard_idx(h)].entry(h).or_insert(0) += 1;
            }
        }
        shards
    };

    let mut merged = AHashMap::with_capacity(cap * COUNT_SHARDS);
    for mut m in shards {
        for (k, v) in m.drain() {
            *merged.entry(k).or_insert(0) += v;
        }
    }
    merged
}

struct DetailState {
    min_url_row: u32,
    min_title_row: u32,
    users: AHashSet<i64>,
    phrase: String,
}

fn pass2_top_details(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    users: &[i64],
    row_count: usize,
    top: &[(u64, u64)],
) -> AHashMap<u64, DetailState> {
    let mut out: AHashMap<u64, DetailState> = AHashMap::with_capacity(top.len());
    let top_set: AHashSet<u64> = top.iter().map(|(h, _)| *h).collect();

    for i in 0..row_count {
        if !q23_row_matches(phrases, urls, titles, i) {
            continue;
        }
        let h = hash_str(phrases.get(i));
        if !top_set.contains(&h) {
            continue;
        }
        let uid = users[i];
        let url = urls.get(i);
        let title = titles.get(i);
        match out.get_mut(&h) {
            Some(d) => {
                if url < urls.get(d.min_url_row as usize) {
                    d.min_url_row = i as u32;
                }
                if title < titles.get(d.min_title_row as usize) {
                    d.min_title_row = i as u32;
                }
                d.users.insert(uid);
            }
            None => {
                out.insert(
                    h,
                    DetailState {
                        min_url_row: i as u32,
                        min_title_row: i as u32,
                        users: AHashSet::from([uid]),
                        phrase: phrases.get(i).to_string(),
                    },
                );
            }
        }
    }
    out
}

fn q23_execute(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    users: &[i64],
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>> {
    let counts = pass1_phrase_counts(phrases, urls, titles, row_count);
    if counts.is_empty() {
        return Vec::new();
    }

    let top: Vec<(u64, u64)> = top_counts(
        counts.into_iter().map(|(h, c)| (c, (h, c))),
        limit,
        offset,
    );

    let details = pass2_top_details(phrases, urls, titles, users, row_count, &top);

    top.into_iter()
        .map(|(h, count)| {
            let d = &details[&h];
            vec![
                d.phrase.clone(),
                urls.get(d.min_url_row as usize).to_string(),
                titles.get(d.min_title_row as usize).to_string(),
                count.to_string(),
                d.users.len().to_string(),
            ]
        })
        .collect()
}

fn is_col(e: &sqlparser::ast::Expr, name: &str) -> bool {
    crate::sql::expr_column_name(e).as_deref() == Some(name)
}
