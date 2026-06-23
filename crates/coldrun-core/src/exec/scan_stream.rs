//! Disk-streaming scan kernels — one column decode at a time, no warm-cache blowup.

use std::collections::HashSet;
use std::mem::size_of;
use std::time::Instant;

use crate::expr::eval_like_match;
use crate::sql::q24_narrow_load;
use crate::sql::ParsedQuery;
use crate::storage::Database;
use crate::Result;

use super::scan_fast::{url_like_pattern, where_is_url_like};
use super::QueryResult;

const Q24_STREAM_COLS: &[&str] = &["URL", "EventTime"];

struct Q24ScanStats {
    indices: Vec<usize>,
    rows_tested: usize,
    rows_matched: usize,
    blocks_read: usize,
    url_comp_bytes: u64,
    event_comp_bytes: u64,
    url_dec_bytes: u64,
    event_dec_bytes: u64,
}

/// Q24: `SELECT * … WHERE URL LIKE … ORDER BY EventTime LIMIT n` without loading URL into cache.
pub fn try_execute_q24_streaming(
    db: &mut Database,
    parsed: &ParsedQuery,
) -> Result<Option<QueryResult>> {
    if !q24_narrow_load(parsed) {
        return Ok(None);
    }
    let Some(where_expr) = parsed.where_expr.as_ref() else {
        return Ok(None);
    };
    if !where_is_url_like(where_expr) {
        return Ok(None);
    }

    let table = db.ensure_hits_meta()?;
    table.drop_columns(Q24_STREAM_COLS);

    let row_count = table.row_count() as usize;
    let pattern = url_like_pattern(where_expr)?;

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(10);
    let need = offset.saturating_add(limit);

    let t_total = Instant::now();
    let t_scan = Instant::now();
    let stats = streaming_topk_url_like_blocks(table, row_count, need, &pattern)?;
    let scan_ms = t_scan.elapsed().as_secs_f64() * 1000.0;

    // Release any decoded columns before projecting SELECT * (one LZ4 decode at a time).
    table.retain_columns(&HashSet::new());

    let t_project = Instant::now();
    let slice: Vec<usize> = stats.indices.into_iter().skip(offset).take(limit).collect();
    let (names, rows) = table.project_rows(&slice)?;
    let project_ms = t_project.elapsed().as_secs_f64() * 1000.0;
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    eprintln!(
        "perf:q24 rows_tested={} rows_matched={} rows_materialized={} blocks_read={} bytes_comp={{URL:{},EventTime:{}}} bytes_dec={{URL:{},EventTime:{}}} phase_ms={{scan:{:.1},project:{:.1},total:{:.1}}}",
        stats.rows_tested,
        stats.rows_matched,
        rows.len(),
        stats.blocks_read,
        stats.url_comp_bytes,
        stats.event_comp_bytes,
        stats.url_dec_bytes,
        stats.event_dec_bytes,
        scan_ms,
        project_ms,
        total_ms
    );

    Ok(Some(QueryResult { columns: names, rows }))
}

fn streaming_topk_url_like_blocks(
    table: &crate::storage::Table,
    row_count: usize,
    need: usize,
    pattern: &str,
) -> Result<Q24ScanStats> {
    use std::collections::BinaryHeap;

    if need == 0 {
        return Ok(Q24ScanStats {
            indices: Vec::new(),
            rows_tested: 0,
            rows_matched: 0,
            blocks_read: 0,
            url_comp_bytes: 0,
            event_comp_bytes: 0,
            url_dec_bytes: 0,
            event_dec_bytes: 0,
        });
    }

    let url_reader = table.column_block_reader("URL")?;
    let event_reader = table.column_block_reader("EventTime")?;
    if url_reader.row_count() != event_reader.row_count() {
        return Err(crate::Error::msg(
            "q24 block readers row_count mismatch between URL and EventTime",
        ));
    }
    if url_reader.row_count() != row_count {
        return Err(crate::Error::msg("q24 block reader row_count mismatch with table"));
    }
    if url_reader.blocks().len() != event_reader.blocks().len() {
        return Err(crate::Error::msg(
            "q24 block readers block count mismatch between URL and EventTime",
        ));
    }

    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    let mut rows_tested = 0usize;
    let mut rows_matched = 0usize;
    let mut url_comp = 0u64;
    let mut event_comp = 0u64;
    let mut url_dec = 0u64;
    let mut event_dec = 0u64;
    let mut blocks = 0usize;

    for (url_meta, event_meta) in url_reader.iter_blocks().zip(event_reader.iter_blocks()) {
        if url_meta.block_id != event_meta.block_id
            || url_meta.row_start != event_meta.row_start
            || url_meta.row_count != event_meta.row_count
        {
            return Err(crate::Error::msg(
                "q24 block metadata mismatch between URL and EventTime",
            ));
        }
        let url_block = url_reader.read_block(url_meta.block_id)?;
        let event_block = event_reader.read_block(event_meta.block_id)?;
        let (matched, tested) = scan_q24_block(
            url_meta.row_start,
            &url_block.bytes,
            &event_block.bytes,
            pattern,
            need,
            &mut heap,
        )?;
        rows_tested += tested;
        rows_matched += matched;
        url_comp += url_meta.compressed_len;
        event_comp += event_meta.compressed_len;
        url_dec += url_block.bytes.len() as u64;
        event_dec += event_block.bytes.len() as u64;
        blocks += 2;
    }

    let mut v: Vec<_> = heap.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Q24ScanStats {
        indices: v.into_iter().map(|(_, i)| i).collect(),
        rows_tested,
        rows_matched,
        blocks_read: blocks,
        url_comp_bytes: url_comp,
        event_comp_bytes: event_comp,
        url_dec_bytes: url_dec,
        event_dec_bytes: event_dec,
    })
}

fn scan_q24_block(
    row_start: usize,
    url_payload: &[u8],
    event_payload: &[u8],
    pattern: &str,
    need: usize,
    heap: &mut std::collections::BinaryHeap<(i64, usize)>,
) -> Result<(usize, usize)> {
    if url_payload.len() < 8 || event_payload.len() < 8 {
        return Err(crate::Error::msg("q24 block payload truncated"));
    }
    let url_rows = u64::from_le_bytes(url_payload[0..8].try_into().unwrap()) as usize;
    let event_rows = u64::from_le_bytes(event_payload[0..8].try_into().unwrap()) as usize;
    if url_rows != event_rows {
        return Err(crate::Error::msg(
            "q24 block row_count mismatch between URL and EventTime",
        ));
    }
    let url_body = &url_payload[8..];
    let event_body = &event_payload[8..];
    if event_body.len() < url_rows * size_of::<i64>() {
        return Err(crate::Error::msg("q24 event block payload truncated"));
    }

    let mut matched = 0usize;
    let mut pos = 0usize;
    for local_row in 0..url_rows {
        if pos + 4 > url_body.len() {
            return Err(crate::Error::msg("q24 url block payload truncated"));
        }
        let len = u32::from_le_bytes(url_body[pos..pos + 4].try_into().unwrap()) as usize;
        let start = pos + 4;
        let end = start + len;
        if end > url_body.len() {
            return Err(crate::Error::msg("q24 url block payload truncated"));
        }
        let s = std::str::from_utf8(&url_body[start..end])
            .map_err(|e| crate::Error::msg(format!("invalid utf8 in q24 url block: {e}")))?;
        pos = end;

        if eval_like_match(s, pattern) {
            matched += 1;
            let off = local_row * size_of::<i64>();
            let k = unsafe {
                std::ptr::read_unaligned(event_body[off..off + size_of::<i64>()].as_ptr() as *const i64)
            };
            let row = row_start + local_row;
            if heap.len() < need {
                heap.push((k, row));
            } else if let Some(&(worst_k, _)) = heap.peek() {
                if k < worst_k {
                    heap.pop();
                    heap.push((k, row));
                }
            }
        }
    }

    Ok((matched, url_rows))
}
