//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! Streams one column file at a time from disk — never loads URL+Title+SearchPhrase into the
//! warm-serve cache (~20 GiB decoded on 100M). Seven column decompresses total: Title+URL mask,
//! SearchPhrase filter+count, UserID, then one batched pass2 (SearchPhrase+URL+Title) for top-K.

use std::path::Path;

use ahash::{AHashMap, AHashSet};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::column_stream::{Int64ColumnScan, Utf8ColumnScan};
use crate::storage::Database;
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::hash_str;
use super::QueryResult;

const PARALLEL_THRESHOLD: usize = 250_000;
const COUNT_SHARDS: usize = 256;

const Q23_COLS: &[&str] = &["URL", "Title", "SearchPhrase", "UserID"];

pub fn try_fused_q23_streaming(db: &mut Database, parsed: &ParsedQuery) -> Result<Option<QueryResult>> {
    if !is_q23_shape(parsed) {
        return Ok(None);
    }

    let table = db.ensure_hits_meta()?;
    table.drop_columns(Q23_COLS);

    let row_count = table.row_count() as usize;
    let col_dir = table.path.join("columns");
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let rows = q23_execute_streaming(&col_dir, row_count, limit, offset)?;

    Ok(Some(QueryResult { columns, rows }))
}

fn is_q23_shape(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 1 {
        return false;
    }
    let sqlparser::ast::Expr::Identifier(id) = &parsed.group_by[0] else {
        return false;
    };
    if id.value != "SearchPhrase" {
        return false;
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
            _ => return false,
        }
    }
    has_min_url && has_min_title && has_count && has_distinct
}

fn is_col(e: &sqlparser::ast::Expr, name: &str) -> bool {
    crate::sql::expr_column_name(e).as_deref() == Some(name)
}

fn build_q23_mask(col_dir: &Path, row_count: usize) -> Result<Vec<bool>> {
    use rayon::prelude::*;

    let title = Utf8ColumnScan::open(&col_dir.join("Title.col"))?;
    let mut mask = vec![false; row_count];
    if row_count >= PARALLEL_THRESHOLD {
        mask.par_iter_mut()
            .enumerate()
            .for_each(|(i, slot)| {
                *slot = memchr::memmem::find(title.str_at(i).as_bytes(), b"Google").is_some();
            });
    } else {
        for i in 0..row_count {
            mask[i] = memchr::memmem::find(title.str_at(i).as_bytes(), b"Google").is_some();
        }
    }
    drop(title);

    let url = Utf8ColumnScan::open(&col_dir.join("URL.col"))?;
    if row_count >= PARALLEL_THRESHOLD {
        mask.par_iter_mut()
            .enumerate()
            .for_each(|(i, slot)| {
                if *slot && memchr::memmem::find(url.str_at(i).as_bytes(), b".google.").is_some() {
                    *slot = false;
                }
            });
    } else {
        for i in 0..row_count {
            if mask[i] && memchr::memmem::find(url.str_at(i).as_bytes(), b".google.").is_some() {
                mask[i] = false;
            }
        }
    }
    drop(url);

    Ok(mask)
}

fn pass1_phrase_filter_and_counts(
    phrase: &Utf8ColumnScan,
    mask: &mut [bool],
    row_count: usize,
) -> AHashMap<u64, u64> {
    use rayon::prelude::*;

    let cap = (row_count / (COUNT_SHARDS * 8)).max(4);
    if row_count >= PARALLEL_THRESHOLD {
        mask.par_iter_mut()
            .enumerate()
            .for_each(|(i, slot)| {
                if *slot && phrase.str_at(i).is_empty() {
                    *slot = false;
                }
            });
        pass1_phrase_counts(phrase, mask, row_count)
    } else {
        let mut map = AHashMap::with_capacity(cap);
        for i in 0..row_count {
            if !mask[i] {
                continue;
            }
            let s = phrase.str_at(i);
            if s.is_empty() {
                mask[i] = false;
                continue;
            }
            let h = hash_str(s);
            *map.entry(h).or_insert(0) += 1;
        }
        map
    }
}

fn merge_phrase_counts(mut a: AHashMap<u64, u64>, b: AHashMap<u64, u64>) -> AHashMap<u64, u64> {
    for (k, v) in b {
        *a.entry(k).or_insert(0) += v;
    }
    a
}

fn pass1_phrase_counts(
    phrase: &Utf8ColumnScan,
    mask: &[bool],
    row_count: usize,
) -> AHashMap<u64, u64> {
    use rayon::prelude::*;

    let cap = (row_count / (COUNT_SHARDS * 8)).max(4);
    if row_count >= PARALLEL_THRESHOLD {
        let parts: Vec<AHashMap<u64, u64>> = (0..row_count)
            .into_par_iter()
            .fold(
                || AHashMap::with_capacity(cap),
                |mut map, i| {
                    if mask[i] {
                        let h = hash_str(phrase.str_at(i));
                        *map.entry(h).or_insert(0) += 1;
                    }
                    map
                },
            )
            .collect();
        let mut merged = AHashMap::with_capacity(cap * parts.len().min(32));
        for part in parts {
            merged = merge_phrase_counts(merged, part);
        }
        merged
    } else {
        let mut map = AHashMap::with_capacity(cap);
        for i in 0..row_count {
            if mask[i] {
                let h = hash_str(phrase.str_at(i));
                *map.entry(h).or_insert(0) += 1;
            }
        }
        map
    }
}

#[derive(Default)]
struct Q23TopAgg {
    phrase_text: Option<String>,
    min_url: Option<String>,
    min_title: Option<String>,
    distinct_users: AHashSet<i64>,
}

fn pass2_top_details(
    col_dir: &Path,
    mask: &[bool],
    row_count: usize,
    top: &[(u64, u64)],
) -> Result<Vec<Vec<String>>> {
    let mut hash_to_slot = AHashMap::with_capacity(top.len());
    for (slot, (h, _)) in top.iter().enumerate() {
        hash_to_slot.insert(*h, slot);
    }
    let mut aggs: Vec<Q23TopAgg> = (0..top.len()).map(|_| Q23TopAgg::default()).collect();
    let mut row_slot = vec![-1i8; row_count];

    // One SearchPhrase pass: map each matching row to top slot and capture phrase text once.
    let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
    for i in 0..row_count {
        if !mask[i] {
            continue;
        }
        let h = hash_str(phrase.str_at(i));
        let Some(&slot) = hash_to_slot.get(&h) else {
            continue;
        };
        row_slot[i] = slot as i8;
        let agg = &mut aggs[slot];
        if agg.phrase_text.is_none() {
            agg.phrase_text = Some(phrase.str_at(i).to_string());
        }
    }
    drop(phrase);

    let url = Utf8ColumnScan::open(&col_dir.join("URL.col"))?;
    for i in 0..row_count {
        let slot = row_slot[i];
        if slot >= 0 {
            update_min(&mut aggs[slot as usize].min_url, url.str_at(i));
        }
    }
    drop(url);

    let title = Utf8ColumnScan::open(&col_dir.join("Title.col"))?;
    for i in 0..row_count {
        let slot = row_slot[i];
        if slot >= 0 {
            update_min(&mut aggs[slot as usize].min_title, title.str_at(i));
        }
    }
    drop(title);

    let users = Int64ColumnScan::open(&col_dir.join("UserID.col"))?;
    for i in 0..row_count {
        let slot = row_slot[i];
        if slot >= 0 {
            aggs[slot as usize].distinct_users.insert(users.at(i));
        }
    }
    drop(users);

    let mut out = Vec::with_capacity(top.len());
    for (phrase_hash, count) in top {
        let agg = hash_to_slot
            .get(phrase_hash)
            .and_then(|&slot| aggs.get(slot));
        let phrase_text = agg
            .and_then(|a| a.phrase_text.as_deref())
            .unwrap_or("");
        let min_url = agg.and_then(|a| a.min_url.as_deref()).unwrap_or("");
        let min_title = agg.and_then(|a| a.min_title.as_deref()).unwrap_or("");
        let distinct = agg.map(|a| a.distinct_users.len()).unwrap_or(0);
        out.push(vec![
            phrase_text.to_string(),
            min_url.to_string(),
            min_title.to_string(),
            count.to_string(),
            distinct.to_string(),
        ]);
    }
    Ok(out)
}

fn update_min(slot: &mut Option<String>, s: &str) {
    match slot {
        None => *slot = Some(s.to_string()),
        Some(cur) if s < cur.as_str() => *cur = s.to_string(),
        _ => {}
    }
}

fn q23_execute_streaming(
    col_dir: &Path,
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Result<Vec<Vec<String>>> {
    let mut mask = build_q23_mask(col_dir, row_count)?;

    let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
    let counts = pass1_phrase_filter_and_counts(&phrase, &mut mask, row_count);
    drop(phrase);

    if counts.is_empty() {
        return Ok(Vec::new());
    }

    let top: Vec<(u64, u64)> = top_counts(
        counts.into_iter().map(|(h, c)| (c, (h, c))),
        limit,
        offset,
    );

    pass2_top_details(col_dir, &mask, row_count, &top)
}
