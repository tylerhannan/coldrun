//! Disk-streaming scan kernels — one column decode at a time, no warm-cache blowup.

use std::collections::HashSet;

use crate::expr::eval_like_match;
use crate::sql::q24_narrow_load;
use crate::sql::ParsedQuery;
use crate::storage::column_stream::{Int64ColumnScan, Utf8ColumnScan};
use crate::storage::Database;
use crate::Result;

use super::scan_fast::{url_like_pattern, where_is_url_like};
use super::QueryResult;

const Q24_STREAM_COLS: &[&str] = &["URL", "EventTime"];
const PARALLEL_THRESHOLD: usize = 250_000;

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

    let times = Int64ColumnScan::open(&col_dir.join("EventTime.col"))?;
    let url = Utf8ColumnScan::open(&col_dir.join("URL.col"))?;
    let indices = streaming_topk_url_like(&url, &times, row_count, need, &pattern);
    drop(url);
    drop(times);

    // Release any decoded columns before projecting SELECT * (batched parallel LZ4 per column).
    table.retain_columns(&HashSet::new());

    let slice: Vec<usize> = indices.into_iter().skip(offset).take(limit).collect();
    let (names, rows) = table.project_rows(&slice)?;

    Ok(Some(QueryResult { columns: names, rows }))
}

fn streaming_topk_url_like(
    url: &Utf8ColumnScan,
    times: &Int64ColumnScan,
    row_count: usize,
    need: usize,
    pattern: &str,
) -> Vec<usize> {
    if need == 0 {
        return Vec::new();
    }
    if row_count >= PARALLEL_THRESHOLD {
        streaming_topk_url_like_parallel(url, times, row_count, need, pattern)
    } else {
        streaming_topk_url_like_serial(url, times, row_count, need, pattern)
    }
}

fn streaming_topk_url_like_serial(
    url: &Utf8ColumnScan,
    times: &Int64ColumnScan,
    row_count: usize,
    need: usize,
    pattern: &str,
) -> Vec<usize> {
    use std::collections::BinaryHeap;

    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    for i in 0..row_count {
        if !eval_like_match(url.str_at(i), pattern) {
            continue;
        }
        push_event_topk(&mut heap, times.at(i), i, need);
    }
    heap_into_sorted_indices(heap)
}

fn streaming_topk_url_like_parallel(
    url: &Utf8ColumnScan,
    times: &Int64ColumnScan,
    row_count: usize,
    need: usize,
    pattern: &str,
) -> Vec<usize> {
    use rayon::prelude::*;

    let n_threads = rayon::current_num_threads().max(1);
    let chunk = row_count.div_ceil(n_threads);

    let partials: Vec<Vec<(i64, usize)>> = (0..n_threads)
        .into_par_iter()
        .map(|tid| {
            let start = tid * chunk;
            if start >= row_count {
                return Vec::new();
            }
            let end = (start + chunk).min(row_count);
            local_topk_url_like(url, times, start, end, need, pattern)
        })
        .collect();

    merge_topk_by_time(partials, need)
}

fn local_topk_url_like(
    url: &Utf8ColumnScan,
    times: &Int64ColumnScan,
    start: usize,
    end: usize,
    need: usize,
    pattern: &str,
) -> Vec<(i64, usize)> {
    use std::collections::BinaryHeap;

    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    for i in start..end {
        if !eval_like_match(url.str_at(i), pattern) {
            continue;
        }
        push_event_topk(&mut heap, times.at(i), i, need);
    }
    let mut v: Vec<_> = heap.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn push_event_topk(heap: &mut std::collections::BinaryHeap<(i64, usize)>, k: i64, i: usize, need: usize) {
    if heap.len() < need {
        heap.push((k, i));
    } else if let Some(&(worst_k, _)) = heap.peek() {
        if k < worst_k {
            heap.pop();
            heap.push((k, i));
        }
    }
}

fn heap_into_sorted_indices(heap: std::collections::BinaryHeap<(i64, usize)>) -> Vec<usize> {
    let mut v: Vec<_> = heap.into_iter().collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.into_iter().map(|(_, i)| i).collect()
}

fn merge_topk_by_time(parts: Vec<Vec<(i64, usize)>>, need: usize) -> Vec<usize> {
    let mut all: Vec<(i64, usize)> = parts.into_iter().flatten().collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    all.into_iter().take(need).map(|(_, i)| i).collect()
}
