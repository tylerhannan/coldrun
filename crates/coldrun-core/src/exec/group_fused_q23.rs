//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! Phase 1: collect matching `(phrase_hash, user)` pairs, sort, top-K by count + distinct users.
//! Phase 2: second scan for top phrases only — MIN(URL), MIN(Title), resolve phrase text.
//! Avoids per-group `AHashSet` maps that OOM on 100M rows.

use ahash::{AHashMap, AHashSet};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_sort::sorted_topk_phrase_user_counts;
use super::group_fused::hash_str;
use super::QueryResult;

const PARALLEL_THRESHOLD: usize = 250_000;

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

struct MinBucket {
    phrase: String,
    min_url: String,
    min_title: String,
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
    let mut pairs = collect_phrase_user_pairs(phrases, urls, titles, users, row_count);
    if pairs.is_empty() {
        return Vec::new();
    }

    let top = sorted_topk_phrase_user_counts(&mut pairs, limit, offset);
    drop(pairs);

    let top_hashes: AHashSet<u64> = top.iter().map(|(h, _, _)| *h).collect();
    let mut mins: AHashMap<u64, MinBucket> = AHashMap::with_capacity(top.len());

    for i in 0..row_count {
        if !q23_row_matches(phrases, urls, titles, i) {
            continue;
        }
        let h = hash_str(phrases.get(i));
        if !top_hashes.contains(&h) {
            continue;
        }
        let url = urls.get(i);
        let title = titles.get(i);
        mins
            .entry(h)
            .and_modify(|b| {
                if url < b.min_url.as_str() {
                    b.min_url = url.to_string();
                }
                if title < b.min_title.as_str() {
                    b.min_title = title.to_string();
                }
            })
            .or_insert(MinBucket {
                phrase: phrases.get(i).to_string(),
                min_url: url.to_string(),
                min_title: title.to_string(),
            });
    }

    top.into_iter()
        .map(|(h, count, distinct)| {
            let b = &mins[&h];
            vec![
                b.phrase.clone(),
                b.min_url.clone(),
                b.min_title.clone(),
                count.to_string(),
                distinct.to_string(),
            ]
        })
        .collect()
}

fn collect_phrase_user_pairs(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    users: &[i64],
    row_count: usize,
) -> Vec<(u64, i64)> {
    use rayon::prelude::*;

    if row_count >= PARALLEL_THRESHOLD {
        (0..row_count)
            .into_par_iter()
            .filter(|&i| q23_row_matches(phrases, urls, titles, i))
            .map(|i| (hash_str(phrases.get(i)), users[i]))
            .collect()
    } else {
        let mut v = Vec::new();
        for i in 0..row_count {
            if q23_row_matches(phrases, urls, titles, i) {
                v.push((hash_str(phrases.get(i)), users[i]));
            }
        }
        v
    }
}

fn is_col(e: &sqlparser::ast::Expr, name: &str) -> bool {
    crate::sql::expr_column_name(e).as_deref() == Some(name)
}
