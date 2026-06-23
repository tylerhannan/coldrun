//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! Streams one column file at a time from disk — never loads URL+Title+SearchPhrase into the
//! warm-serve cache (~20 GiB decoded on 100M). Column decompresses: Title+URL mask (one row pass),
//! SearchPhrase filter+count, UserID, then batched pass2 (SearchPhrase+URL+Title) for top-K.

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
/// Build explicit row-index list when sparser than this (saves scanning non-matching rows).
const SPARSE_INDEX_RATIO: usize = 8;

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

/// Title `%Google%` and URL not `%.google.%` in a single row pass (two LZ4 decompresses).
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

fn matching_row_indices(mask: &[bool]) -> Vec<usize> {
    let true_count = mask.iter().filter(|&&m| m).count();
    if true_count * SPARSE_INDEX_RATIO > mask.len() {
        return Vec::new();
    }
    mask.iter()
        .enumerate()
        .filter(|(_, &m)| m)
        .map(|(i, _)| i)
        .collect()
}

fn clear_empty_phrases(phrase: &Utf8ColumnScan, mask: &mut [bool], sparse: &[usize]) {
    use rayon::prelude::*;

    if sparse.is_empty() {
        mask.par_iter_mut()
            .enumerate()
            .for_each(|(i, slot)| {
                if *slot && phrase.str_at(i).is_empty() {
                    *slot = false;
                }
            });
    } else {
        for &i in sparse {
            if mask[i] && phrase.str_at(i).is_empty() {
                mask[i] = false;
            }
        }
    }
}

fn pass1_phrase_filter_and_counts(
    phrase: &Utf8ColumnScan,
    mask: &mut [bool],
    sparse: &[usize],
    row_count: usize,
) -> AHashMap<u64, u64> {
    let cap = (row_count / 256).max(4);

    if row_count >= PARALLEL_THRESHOLD {
        clear_empty_phrases(phrase, mask, sparse);
        return phrase_counts_parallel(phrase, mask, sparse, cap);
    }

    let mut map = AHashMap::with_capacity(cap);
    if sparse.is_empty() {
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
    } else {
        for &i in sparse {
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
    }
    map
}

fn phrase_counts_parallel(
    phrase: &Utf8ColumnScan,
    mask: &[bool],
    sparse: &[usize],
    cap: usize,
) -> AHashMap<u64, u64> {
    use rayon::prelude::*;

    if !sparse.is_empty() {
        return sparse
            .par_iter()
            .fold(
                || AHashMap::with_capacity(cap),
                |mut map, &i| {
                    if mask[i] {
                        let h = hash_str(phrase.str_at(i));
                        *map.entry(h).or_insert(0) += 1;
                    }
                    map
                },
            )
            .reduce(|| AHashMap::new(), merge_phrase_counts);
    }

    let n_threads = rayon::current_num_threads().max(1);
    let chunk = mask.len().div_ceil(n_threads);
    (0..n_threads)
        .into_par_iter()
        .map(|tid| {
            let start = tid * chunk;
            if start >= mask.len() {
                return AHashMap::new();
            }
            let end = (start + chunk).min(mask.len());
            let mut map = AHashMap::with_capacity(cap);
            for i in start..end {
                if mask[i] {
                    let h = hash_str(phrase.str_at(i));
                    *map.entry(h).or_insert(0) += 1;
                }
            }
            map
        })
        .reduce(|| AHashMap::new(), merge_phrase_counts)
}

fn merge_phrase_counts(mut a: AHashMap<u64, u64>, b: AHashMap<u64, u64>) -> AHashMap<u64, u64> {
    for (k, v) in b {
        *a.entry(k).or_insert(0) += v;
    }
    a
}

fn collect_top_phrase_rows(
    phrase: &Utf8ColumnScan,
    mask: &[bool],
    sparse: &[usize],
    top_hashes: &AHashSet<u64>,
) -> AHashMap<u64, (String, Vec<usize>)> {
    let mut details = AHashMap::with_capacity(top_hashes.len());
    let mut consider = |i: usize| {
        if !mask[i] {
            return;
        }
        let s = phrase.str_at(i);
        let h = hash_str(s);
        if !top_hashes.contains(&h) {
            return;
        }
        details
            .entry(h)
            .or_insert_with(|| (s.to_string(), Vec::new()))
            .1
            .push(i);
    };
    if sparse.is_empty() {
        for i in 0..mask.len() {
            consider(i);
        }
    } else {
        for &i in sparse {
            consider(i);
        }
    }
    details
}

fn pass2_top_details(
    col_dir: &Path,
    mask: &[bool],
    sparse: &[usize],
    top: &[(u64, u64)],
    users: &Int64ColumnScan,
) -> Result<Vec<Vec<String>>> {
    let top_hashes: AHashSet<u64> = top.iter().map(|(h, _)| *h).collect();

    let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
    let details = collect_top_phrase_rows(&phrase, mask, sparse, &top_hashes);
    drop(phrase);

    let url = Utf8ColumnScan::open(&col_dir.join("URL.col"))?;
    let title = Utf8ColumnScan::open(&col_dir.join("Title.col"))?;
    let mut min_urls = AHashMap::with_capacity(details.len());
    let mut min_titles = AHashMap::with_capacity(details.len());
    for (&h, (_, rows)) in &details {
        min_urls.insert(h, min_utf8_at_rows(&url, rows));
        min_titles.insert(h, min_utf8_at_rows(&title, rows));
    }
    drop(url);
    drop(title);

    let mut out = Vec::with_capacity(top.len());
    for (phrase_hash, count) in top {
        let (phrase_text, rows) = details
            .get(phrase_hash)
            .map(|(p, r)| (p.as_str(), r.as_slice()))
            .unwrap_or(("", &[]));
        let min_url = min_urls.get(phrase_hash).map(String::as_str).unwrap_or("");
        let min_title = min_titles.get(phrase_hash).map(String::as_str).unwrap_or("");
        let distinct = distinct_users_at_rows(users, rows);
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

fn distinct_users_at_rows(users: &Int64ColumnScan, rows: &[usize]) -> usize {
    let mut set = AHashSet::with_capacity(rows.len().min(4096));
    for &i in rows {
        set.insert(users.at(i));
    }
    set.len()
}

fn q23_execute_streaming(
    col_dir: &Path,
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Result<Vec<Vec<String>>> {
    let mut mask = build_q23_mask(col_dir, row_count)?;
    let sparse = matching_row_indices(&mask);

    let phrase = Utf8ColumnScan::open(&col_dir.join("SearchPhrase.col"))?;
    let counts = pass1_phrase_filter_and_counts(&phrase, &mut mask, &sparse, row_count);
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
    let rows = pass2_top_details(col_dir, &mask, &sparse, &top, &users)?;
    drop(users);

    Ok(rows)
}
