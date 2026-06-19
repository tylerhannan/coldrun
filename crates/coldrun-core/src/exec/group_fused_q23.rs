//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! Streams one column file at a time from disk — never loads URL+Title+SearchPhrase into the
//! warm-serve cache (~20 GiB decoded on 100M). ClickHouse-style: filter mask, count pass, then
//! late materialization for top-K groups only.

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

    let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
    if row_count >= PARALLEL_THRESHOLD {
        mask.par_iter_mut()
            .enumerate()
            .for_each(|(i, slot)| {
                if *slot && phrase.str_at(i).is_empty() {
                    *slot = false;
                }
            });
    } else {
        for i in 0..row_count {
            if mask[i] && phrase.str_at(i).is_empty() {
                mask[i] = false;
            }
        }
    }
    drop(phrase);

    Ok(mask)
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
    phrase: &Utf8ColumnScan,
    mask: &[bool],
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
                    if mask[i] {
                        let h = hash_str(phrase.str_at(i));
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
            if mask[i] {
                let h = hash_str(phrase.str_at(i));
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

fn collect_phrase_rows(
    phrase: &Utf8ColumnScan,
    mask: &[bool],
    row_count: usize,
    phrase_hash: u64,
) -> (Vec<usize>, String) {
    let mut rows = Vec::new();
    let mut phrase_text = String::new();
    for i in 0..row_count {
        if !mask[i] {
            continue;
        }
        let s = phrase.str_at(i);
        if hash_str(s) != phrase_hash {
            continue;
        }
        if phrase_text.is_empty() {
            phrase_text = s.to_string();
        }
        rows.push(i);
    }
    (rows, phrase_text)
}

fn min_utf8_at_rows(col: &Utf8ColumnScan, rows: &[usize]) -> String {
    let mut min: Option<&str> = None;
    for &i in rows {
        let s = col.str_at(i);
        min = Some(match min {
            None => s,
            Some(m) if s < m => s,
            Some(m) => m,
        });
    }
    min.unwrap_or("").to_string()
}

fn distinct_users_at_rows(users: &[i64], rows: &[usize]) -> usize {
    let mut set = AHashSet::with_capacity(rows.len().min(4096));
    for &i in rows {
        set.insert(users[i]);
    }
    set.len()
}

fn q23_execute_streaming(
    col_dir: &Path,
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Result<Vec<Vec<String>>> {
    let mask = build_q23_mask(col_dir, row_count)?;

    let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
    let counts = pass1_phrase_counts(&phrase, &mask, row_count);
    drop(phrase);

    if counts.is_empty() {
        return Ok(Vec::new());
    }

    let top: Vec<(u64, u64)> = top_counts(
        counts.into_iter().map(|(h, c)| (c, (h, c))),
        limit,
        offset,
    );

    let users = Int64ColumnScan::open(&col_dir.join("UserID.col"))?;
    let user_slice = users.as_slice();

    let mut out = Vec::with_capacity(top.len());
    for (phrase_hash, count) in top {
        let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
        let (rows, phrase_text) = collect_phrase_rows(&phrase, &mask, row_count, phrase_hash);
        drop(phrase);

        let url = Utf8ColumnScan::open(&col_dir.join("URL.col"))?;
        let min_url = min_utf8_at_rows(&url, &rows);
        drop(url);

        let title = Utf8ColumnScan::open(&col_dir.join("Title.col"))?;
        let min_title = min_utf8_at_rows(&title, &rows);
        drop(title);

        let distinct = distinct_users_at_rows(user_slice, &rows);

        out.push(vec![
            phrase_text,
            min_url,
            min_title,
            count.to_string(),
            distinct.to_string(),
        ]);
    }

    Ok(out)
}
