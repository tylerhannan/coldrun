//! Disk-streaming scan kernels — one column decode at a time, no warm-cache blowup.

use std::collections::HashSet;
use std::time::Instant;

use crate::expr::eval_like_match;
use crate::sql::q24_narrow_load;
use crate::sql::ParsedQuery;
use crate::storage::column_stream::{Int64ColumnScan, Utf8ColumnScan};
use crate::storage::Database;
use crate::Result;

use super::scan_fast::{url_like_pattern, where_is_url_like};
use super::QueryResult;

const Q24_STREAM_COLS: &[&str] = &["URL", "EventTime"];

fn file_bytes(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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
    let col_dir = table.path.join("columns");
    let pattern = url_like_pattern(where_expr)?;

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(10);
    let need = offset.saturating_add(limit);

    let event_time_path = col_dir.join("EventTime.col");
    let url_path = col_dir.join("URL.col");
    let t_total = Instant::now();
    let t_scan = Instant::now();
    let times = Int64ColumnScan::open(&event_time_path)?;
    let url = Utf8ColumnScan::open(&url_path)?;
    let (indices, matched_rows) = streaming_topk_url_like(&url, &times, row_count, need, &pattern);
    let scan_ms = t_scan.elapsed().as_secs_f64() * 1000.0;
    let url_dec = url.decompressed_bytes() as u64;
    let event_dec = times.decompressed_bytes() as u64;
    drop(url);
    drop(times);

    // Release any decoded columns before projecting SELECT * (one LZ4 decode at a time).
    table.retain_columns(&HashSet::new());

    let t_project = Instant::now();
    let slice: Vec<usize> = indices.into_iter().skip(offset).take(limit).collect();
    let (names, rows) = table.project_rows(&slice)?;
    let project_ms = t_project.elapsed().as_secs_f64() * 1000.0;
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    eprintln!(
        "perf:q24 rows_tested={} rows_matched={} rows_materialized={} blocks_read=2 bytes_comp={{URL:{},EventTime:{}}} bytes_dec={{URL:{},EventTime:{}}} phase_ms={{scan:{:.1},project:{:.1},total:{:.1}}}",
        row_count,
        matched_rows,
        rows.len(),
        file_bytes(&url_path),
        file_bytes(&event_time_path),
        url_dec,
        event_dec,
        scan_ms,
        project_ms,
        total_ms
    );

    Ok(Some(QueryResult { columns: names, rows }))
}

fn streaming_topk_url_like(
    url: &Utf8ColumnScan,
    times: &Int64ColumnScan,
    row_count: usize,
    need: usize,
    pattern: &str,
) -> (Vec<usize>, usize) {
    use std::collections::BinaryHeap;

    if need == 0 {
        return (Vec::new(), 0);
    }
    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    let mut matched = 0usize;
    for i in 0..row_count {
        if !eval_like_match(url.str_at(i), pattern) {
            continue;
        }
        matched += 1;
        let k = times.at(i);
        if heap.len() < need {
            heap.push((k, i));
        } else if let Some(&(worst_k, _)) = heap.peek() {
            if k < worst_k {
                heap.pop();
                heap.push((k, i));
            }
        }
    }
    let mut v: Vec<_> = heap.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    (v.into_iter().map(|(_, i)| i).collect(), matched)
}
