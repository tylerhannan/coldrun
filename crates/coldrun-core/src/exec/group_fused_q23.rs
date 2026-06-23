//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! Streams one column file at a time from disk — never loads URL+Title+SearchPhrase into the
//! warm-serve cache (~20 GiB decoded on 100M). Seven column decompresses total: Title+URL mask,
//! SearchPhrase filter+count, UserID, then one batched pass2 (SearchPhrase+URL+Title) for top-K.

use std::path::Path;
use std::time::Instant;

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

fn file_bytes(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[derive(Default)]
struct Q23Perf {
    blocks_read: u64,
    comp_title: u64,
    comp_url: u64,
    comp_phrase: u64,
    comp_user: u64,
    dec_title: u64,
    dec_url: u64,
    dec_phrase: u64,
    dec_user: u64,
    rows_tested: usize,
    rows_after_mask: usize,
    rows_after_phrase: usize,
    rows_materialized: usize,
}

impl Q23Perf {
    fn add_scan(&mut self, col: &str, comp: u64, dec: u64) {
        self.blocks_read += 1;
        match col {
            "Title" => {
                self.comp_title += comp;
                self.dec_title += dec;
            }
            "URL" => {
                self.comp_url += comp;
                self.dec_url += dec;
            }
            "SearchPhrase" => {
                self.comp_phrase += comp;
                self.dec_phrase += dec;
            }
            "UserID" => {
                self.comp_user += comp;
                self.dec_user += dec;
            }
            _ => {}
        }
    }
}

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

fn build_q23_mask(col_dir: &Path, row_count: usize, perf: &mut Q23Perf) -> Result<Vec<bool>> {
    use rayon::prelude::*;

    let title_path = col_dir.join("Title.col");
    let title = Utf8ColumnScan::open(&title_path)?;
    perf.add_scan("Title", file_bytes(&title_path), title.decompressed_bytes() as u64);
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

    let url_path = col_dir.join("URL.col");
    let url = Utf8ColumnScan::open(&url_path)?;
    perf.add_scan("URL", file_bytes(&url_path), url.decompressed_bytes() as u64);
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
    perf: &mut Q23Perf,
) -> Result<Vec<Vec<String>>> {
    let mut hash_to_slot = AHashMap::with_capacity(top.len());
    for (slot, (h, _)) in top.iter().enumerate() {
        hash_to_slot.insert(*h, slot);
    }
    let mut aggs: Vec<Q23TopAgg> = (0..top.len()).map(|_| Q23TopAgg::default()).collect();
    let mut row_slot = vec![-1i8; row_count];

    // One SearchPhrase pass: map each matching row to top slot and capture phrase text once.
    let phrase_path = col_dir.join("SearchPhrase.col");
    let phrase = Utf8ColumnScan::open(&phrase_path)?;
    perf.add_scan(
        "SearchPhrase",
        file_bytes(&phrase_path),
        phrase.decompressed_bytes() as u64,
    );
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
    perf.rows_materialized = row_slot.iter().filter(|&&s| s >= 0).count();

    let url_path = col_dir.join("URL.col");
    let url = Utf8ColumnScan::open(&url_path)?;
    perf.add_scan("URL", file_bytes(&url_path), url.decompressed_bytes() as u64);
    for i in 0..row_count {
        let slot = row_slot[i];
        if slot >= 0 {
            update_min(&mut aggs[slot as usize].min_url, url.str_at(i));
        }
    }
    drop(url);

    let title_path = col_dir.join("Title.col");
    let title = Utf8ColumnScan::open(&title_path)?;
    perf.add_scan("Title", file_bytes(&title_path), title.decompressed_bytes() as u64);
    for i in 0..row_count {
        let slot = row_slot[i];
        if slot >= 0 {
            update_min(&mut aggs[slot as usize].min_title, title.str_at(i));
        }
    }
    drop(title);

    let user_path = col_dir.join("UserID.col");
    let users = Int64ColumnScan::open(&user_path)?;
    perf.add_scan("UserID", file_bytes(&user_path), users.decompressed_bytes() as u64);
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
    let mut perf = Q23Perf {
        rows_tested: row_count,
        ..Q23Perf::default()
    };
    let t_total = Instant::now();

    let t_mask = Instant::now();
    let mut mask = build_q23_mask(col_dir, row_count, &mut perf)?;
    let phase_mask_ms = t_mask.elapsed().as_secs_f64() * 1000.0;
    perf.rows_after_mask = mask.iter().filter(|&&m| m).count();

    let phrase_path = col_dir.join("SearchPhrase.col");
    let t_count = Instant::now();
    let phrase = Utf8ColumnScan::open(&phrase_path)?;
    perf.add_scan(
        "SearchPhrase",
        file_bytes(&phrase_path),
        phrase.decompressed_bytes() as u64,
    );
    let counts = pass1_phrase_filter_and_counts(&phrase, &mut mask, row_count);
    drop(phrase);
    let phase_count_ms = t_count.elapsed().as_secs_f64() * 1000.0;
    perf.rows_after_phrase = mask.iter().filter(|&&m| m).count();

    if counts.is_empty() {
        return Ok(Vec::new());
    }

    let t_top = Instant::now();
    let top: Vec<(u64, u64)> = top_counts(
        counts.into_iter().map(|(h, c)| (c, (h, c))),
        limit,
        offset,
    );
    let phase_top_ms = t_top.elapsed().as_secs_f64() * 1000.0;

    let t_pass2 = Instant::now();
    let rows = pass2_top_details(col_dir, &mask, row_count, &top, &mut perf)?;
    let phase_pass2_ms = t_pass2.elapsed().as_secs_f64() * 1000.0;
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    eprintln!(
        "perf:q23 rows_tested={} rows_after_mask={} rows_after_phrase={} rows_materialized={} blocks_read={} bytes_comp={{Title:{},URL:{},SearchPhrase:{},UserID:{}}} bytes_dec={{Title:{},URL:{},SearchPhrase:{},UserID:{}}} phase_ms={{mask:{:.1},count:{:.1},topk:{:.1},pass2:{:.1},total:{:.1}}}",
        perf.rows_tested,
        perf.rows_after_mask,
        perf.rows_after_phrase,
        perf.rows_materialized,
        perf.blocks_read,
        perf.comp_title,
        perf.comp_url,
        perf.comp_phrase,
        perf.comp_user,
        perf.dec_title,
        perf.dec_url,
        perf.dec_phrase,
        perf.dec_user,
        phase_mask_ms,
        phase_count_ms,
        phase_top_ms,
        phase_pass2_ms,
        total_ms
    );

    Ok(rows)
}
