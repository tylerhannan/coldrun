//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).
//!
//! Streams per-column blocks from disk via the block-reader API — never loads URL+Title+
//! SearchPhrase into the warm-serve cache (~20 GiB decoded on 100M).

use std::time::Instant;

use ahash::{AHashMap, AHashSet};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::Database;
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::hash_str;
use super::QueryResult;

const Q23_COLS: &[&str] = &["URL", "Title", "SearchPhrase", "UserID"];

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
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let rows = q23_execute_streaming(table, row_count, limit, offset)?;

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

fn build_q23_mask(
    table: &crate::storage::Table,
    row_count: usize,
    perf: &mut Q23Perf,
) -> Result<Vec<bool>> {
    let title = table.column_block_reader("Title")?;
    let url = table.column_block_reader("URL")?;
    if title.row_count() != url.row_count() || title.row_count() != row_count {
        return Err(crate::Error::msg(
            "q23 mask block readers row_count mismatch for Title/URL",
        ));
    }
    if title.blocks().len() != url.blocks().len() {
        return Err(crate::Error::msg(
            "q23 mask block readers block count mismatch for Title/URL",
        ));
    }

    let mut mask = vec![false; row_count];
    for (title_meta, url_meta) in title.iter_blocks().zip(url.iter_blocks()) {
        if title_meta.block_id != url_meta.block_id
            || title_meta.row_start != url_meta.row_start
            || title_meta.row_count != url_meta.row_count
        {
            return Err(crate::Error::msg(
                "q23 mask block metadata mismatch for Title/URL",
            ));
        }
        let title_block = title.read_block(title_meta.block_id)?;
        let url_block = url.read_block(url_meta.block_id)?;
        perf.add_scan("Title", title_meta.compressed_len, title_block.bytes.len() as u64);
        perf.add_scan("URL", url_meta.compressed_len, url_block.bytes.len() as u64);
        apply_q23_mask_block(title_meta.row_start, &title_block.bytes, &url_block.bytes, &mut mask)?;
    }

    Ok(mask)
}

fn pass1_phrase_filter_and_counts(
    table: &crate::storage::Table,
    mask: &mut [bool],
    row_count: usize,
    perf: &mut Q23Perf,
) -> Result<AHashMap<u64, u64>> {
    let phrase = table.column_block_reader("SearchPhrase")?;
    if phrase.row_count() != row_count {
        return Err(crate::Error::msg(
            "q23 pass1 row_count mismatch for SearchPhrase",
        ));
    }

    let mut map = AHashMap::with_capacity((row_count / 64).max(4));
    for meta in phrase.iter_blocks() {
        let block = phrase.read_block(meta.block_id)?;
        perf.add_scan("SearchPhrase", meta.compressed_len, block.bytes.len() as u64);
        for_each_utf8_row(meta.row_start, &block.bytes, |row, s| {
            if !mask[row] {
                return Ok(());
            }
            if s.is_empty() {
                mask[row] = false;
                return Ok(());
            }
            *map.entry(hash_str(s)).or_insert(0) += 1;
            Ok(())
        })?;
    }
    Ok(map)
}

#[derive(Default)]
struct Q23TopAgg {
    phrase_text: Option<String>,
    min_url: Option<String>,
    min_title: Option<String>,
    distinct_users: AHashSet<i64>,
}

fn pass2_top_details(
    table: &crate::storage::Table,
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
    let mut row_slot = vec![-1i32; row_count];

    // One SearchPhrase pass: map each matching row to top slot and capture phrase text once.
    let phrase = table.column_block_reader("SearchPhrase")?;
    if phrase.row_count() != row_count {
        return Err(crate::Error::msg(
            "q23 pass2 row_count mismatch for SearchPhrase",
        ));
    }
    for meta in phrase.iter_blocks() {
        let block = phrase.read_block(meta.block_id)?;
        perf.add_scan("SearchPhrase", meta.compressed_len, block.bytes.len() as u64);
        for_each_utf8_row(meta.row_start, &block.bytes, |row, s| {
            if !mask[row] {
                return Ok(());
            }
            let h = hash_str(s);
            let Some(&slot) = hash_to_slot.get(&h) else {
                return Ok(());
            };
            row_slot[row] = slot as i32;
            let agg = &mut aggs[slot];
            if agg.phrase_text.is_none() {
                agg.phrase_text = Some(s.to_string());
            }
            Ok(())
        })?;
    }
    perf.rows_materialized = row_slot.iter().filter(|&&s| s >= 0).count();

    let url = table.column_block_reader("URL")?;
    if url.row_count() != row_count {
        return Err(crate::Error::msg("q23 pass2 row_count mismatch for URL"));
    }
    for meta in url.iter_blocks() {
        let block = url.read_block(meta.block_id)?;
        perf.add_scan("URL", meta.compressed_len, block.bytes.len() as u64);
        for_each_utf8_row(meta.row_start, &block.bytes, |row, s| {
            let slot = row_slot[row];
            if slot >= 0 {
                update_min(&mut aggs[slot as usize].min_url, s);
            }
            Ok(())
        })?;
    }

    let title = table.column_block_reader("Title")?;
    if title.row_count() != row_count {
        return Err(crate::Error::msg("q23 pass2 row_count mismatch for Title"));
    }
    for meta in title.iter_blocks() {
        let block = title.read_block(meta.block_id)?;
        perf.add_scan("Title", meta.compressed_len, block.bytes.len() as u64);
        for_each_utf8_row(meta.row_start, &block.bytes, |row, s| {
            let slot = row_slot[row];
            if slot >= 0 {
                update_min(&mut aggs[slot as usize].min_title, s);
            }
            Ok(())
        })?;
    }

    let users = table.column_block_reader("UserID")?;
    if users.row_count() != row_count {
        return Err(crate::Error::msg("q23 pass2 row_count mismatch for UserID"));
    }
    for meta in users.iter_blocks() {
        let block = users.read_block(meta.block_id)?;
        perf.add_scan("UserID", meta.compressed_len, block.bytes.len() as u64);
        for_each_i64_row(meta.row_start, &block.bytes, |row, user| {
            let slot = row_slot[row];
            if slot >= 0 {
                aggs[slot as usize].distinct_users.insert(user);
            }
            Ok(())
        })?;
    }

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
    table: &crate::storage::Table,
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
    let mut mask = build_q23_mask(table, row_count, &mut perf)?;
    let phase_mask_ms = t_mask.elapsed().as_secs_f64() * 1000.0;
    perf.rows_after_mask = mask.iter().filter(|&&m| m).count();

    let t_count = Instant::now();
    let counts = pass1_phrase_filter_and_counts(table, &mut mask, row_count, &mut perf)?;
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
    let rows = pass2_top_details(table, &mask, row_count, &top, &mut perf)?;
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

fn apply_q23_mask_block(
    row_start: usize,
    title_payload: &[u8],
    url_payload: &[u8],
    mask: &mut [bool],
) -> Result<()> {
    if title_payload.len() < 8 || url_payload.len() < 8 {
        return Err(crate::Error::msg("q23 mask block payload truncated"));
    }
    let title_rows = u64::from_le_bytes(title_payload[0..8].try_into().unwrap()) as usize;
    let url_rows = u64::from_le_bytes(url_payload[0..8].try_into().unwrap()) as usize;
    if title_rows != url_rows {
        return Err(crate::Error::msg(
            "q23 mask block row_count mismatch between Title and URL",
        ));
    }
    let mut tpos = 8usize;
    let mut upos = 8usize;
    for local in 0..title_rows {
        let title = utf8_at(title_payload, &mut tpos)?;
        let url = utf8_at(url_payload, &mut upos)?;
        let row = row_start + local;
        mask[row] = memchr::memmem::find(title.as_bytes(), b"Google").is_some()
            && memchr::memmem::find(url.as_bytes(), b".google.").is_none();
    }
    Ok(())
}

fn for_each_utf8_row<F>(row_start: usize, payload: &[u8], mut f: F) -> Result<()>
where
    F: FnMut(usize, &str) -> Result<()>,
{
    if payload.len() < 8 {
        return Err(crate::Error::msg("q23 utf8 block payload truncated"));
    }
    let rows = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
    let mut pos = 8usize;
    for local in 0..rows {
        let s = utf8_at(payload, &mut pos)?;
        f(row_start + local, s)?;
    }
    Ok(())
}

fn for_each_i64_row<F>(row_start: usize, payload: &[u8], mut f: F) -> Result<()>
where
    F: FnMut(usize, i64) -> Result<()>,
{
    if payload.len() < 8 {
        return Err(crate::Error::msg("q23 i64 block payload truncated"));
    }
    let rows = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
    let body = &payload[8..];
    let bytes = rows * std::mem::size_of::<i64>();
    if body.len() < bytes {
        return Err(crate::Error::msg("q23 i64 block payload truncated"));
    }
    for local in 0..rows {
        let off = local * std::mem::size_of::<i64>();
        let v = unsafe {
            std::ptr::read_unaligned(
                body[off..off + std::mem::size_of::<i64>()].as_ptr() as *const i64,
            )
        };
        f(row_start + local, v)?;
    }
    Ok(())
}

fn utf8_at<'a>(payload: &'a [u8], pos: &mut usize) -> Result<&'a str> {
    if *pos + 4 > payload.len() {
        return Err(crate::Error::msg("q23 utf8 payload truncated"));
    }
    let len = u32::from_le_bytes(payload[*pos..*pos + 4].try_into().unwrap()) as usize;
    let start = *pos + 4;
    let end = start + len;
    if end > payload.len() {
        return Err(crate::Error::msg("q23 utf8 payload truncated"));
    }
    *pos = end;
    std::str::from_utf8(&payload[start..end])
        .map_err(|e| crate::Error::msg(format!("invalid utf8 in q23 block payload: {e}")))
}
