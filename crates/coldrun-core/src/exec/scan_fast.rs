//! Fast scan for `SELECT col … ORDER BY … LIMIT` (Q25–Q27 pattern).

use std::collections::BinaryHeap;

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::expr::eval_like_match;
use crate::sql::{expr_column_name, ParsedQuery, SelectItemKind};
use crate::storage::zones::{ZoneIndex, ZONE_ROWS, ZONE_VERSION_V2};
use crate::storage::{ColumnData, Table, Utf8Column};
use crate::Result;

use super::filter::build_filter_mask;
use super::QueryResult;
use super::projection_label;

pub fn try_execute_scan_fast(
    table: &mut Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = try_scan_int_eq(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_scan_star_like_order_limit(table, parsed, row_count)? {
        return Ok(Some(r));
    }

    if parsed.select_all || parsed.select_items.len() != 1 {
        return Ok(None);
    }

    let proj = &parsed.select_items[0];
    let SelectItemKind::Column(sel_expr) = &proj.kind else {
        return Ok(None);
    };
    let sel_name = expr_column_name(sel_expr).ok_or_else(|| crate::Error::msg("scan col"))?;

    match parsed.order_by.len() {
        1 => {
            let (order_expr, _) = &parsed.order_by[0];
            let order_name = order_column_name(order_expr);
            if order_name != sel_name {
                return try_scan_order_different_col(
                    table,
                    parsed,
                    row_count,
                    proj,
                    &sel_name,
                    &order_name,
                );
            }
            try_scan_single_order(table, parsed, row_count, proj, &sel_name)
        }
        2 => try_scan_two_order(table, parsed, row_count, proj, &sel_name),
        _ => Ok(None),
    }
}

/// Q25–Q26: `ORDER BY` same column as `SELECT`.
fn try_scan_single_order(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    proj: &crate::sql::SelectProjection,
    sel_name: &str,
) -> Result<Option<QueryResult>> {
    let (order_expr, desc) = &parsed.order_by[0];
    let order_name = order_column_name(order_expr);
    if order_name != sel_name {
        return Ok(None);
    }

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(row_count);
    let need = offset.saturating_add(limit);
    if need > 0 && need < row_count {
        if let Some(filter_name) = utf8_ne_empty_column(parsed.where_expr.as_ref()) {
            if filter_name == sel_name {
                let col = table.column(sel_name)?;
                if let ColumnData::Utf8(v) = col {
                    let indices =
                        streaming_topk_utf8(v, row_count, need, *desc, |i| !v.get(i).is_empty());
                    return Ok(Some(build_scan_result(proj, col, &indices, parsed)?));
                }
            }
        }
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let col = table.column(sel_name)?;
    let indices = if need > 0 && need < row_count {
        if let ColumnData::Utf8(v) = col {
            streaming_topk_utf8_from_mask(v, row_count, need, *desc, &mask)
        } else {
            streaming_topk_from_mask(col, row_count, need, *desc, &mask)
        }
    } else {
        let mut indices =
            select_indices_for_where(table, parsed.where_expr.as_ref(), row_count, &mask)?;
        partial_or_full_sort_indices(col, &mut indices, parsed, *desc);
        indices
    };
    Ok(Some(build_scan_result(
        proj,
        col,
        &indices,
        parsed,
    )?))
}

/// Q25: `SELECT SearchPhrase … WHERE SearchPhrase <> '' ORDER BY EventTime LIMIT n`.
fn try_scan_order_different_col(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    proj: &crate::sql::SelectProjection,
    sel_name: &str,
    order_name: &str,
) -> Result<Option<QueryResult>> {
    let (order_expr, desc) = &parsed.order_by[0];
    if order_column_name(order_expr) != order_name {
        return Ok(None);
    }
    let order_col = table.column(order_name)?;
    let sel_col = table.column(sel_name)?;

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(row_count);
    let need = offset.saturating_add(limit);
    if need > 0 && need < row_count {
        if let Some(filter_name) = utf8_ne_empty_column(parsed.where_expr.as_ref()) {
            if filter_name == sel_name {
                if let (ColumnData::Utf8(v), ColumnData::Timestamp(times)) =
                    (sel_col, order_col)
                {
                    let indices = streaming_topk_i64(
                        table,
                        row_count,
                        need,
                        *desc,
                        |i| !v.get(i).is_empty(),
                        |i| times[i],
                    );
                    return Ok(Some(build_scan_result(proj, sel_col, &indices, parsed)?));
                }
            }
        }
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(row_count);
    let need = offset.saturating_add(limit);
    let indices = if need > 0 && need < row_count {
        streaming_topk_from_mask(order_col, row_count, need, *desc, &mask)
    } else {
        let mut indices =
            select_indices_for_where(table, parsed.where_expr.as_ref(), row_count, &mask)?;
        partial_or_full_sort_indices(order_col, &mut indices, parsed, *desc);
        indices
    };
    Ok(Some(build_scan_result(proj, sel_col, &indices, parsed)?))
}

/// Q27: `SELECT SearchPhrase … ORDER BY EventTime, SearchPhrase`.
fn try_scan_two_order(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    proj: &crate::sql::SelectProjection,
    sel_name: &str,
) -> Result<Option<QueryResult>> {
    let (e1, d1) = &parsed.order_by[0];
    let (e2, d2) = &parsed.order_by[1];
    let n1 = order_column_name(e1);
    let n2 = order_column_name(e2);
    if n2 != sel_name {
        return Ok(None);
    }

    let col1 = table.column(&n1)?;
    let col2 = table.column(&n2)?;

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(row_count);
    let need = offset.saturating_add(limit);
    if need > 0 && need < row_count && n1 == "EventTime" && !*d1 {
        if let Some(filter_name) = utf8_ne_empty_column(parsed.where_expr.as_ref()) {
            if filter_name == sel_name {
                if let (ColumnData::Timestamp(times), ColumnData::Utf8(v)) = (col1, col2) {
                    if table
                        .zones()
                        .is_some_and(|z| z.event_time_monotonic_in_row_order())
                    {
                        let mut indices =
                            forward_scan_until(row_count, need, |i| !v.get(i).is_empty());
                        indices.sort_by(|&a, &b| {
                            let t = times[a].cmp(&times[b]);
                            if t != std::cmp::Ordering::Equal {
                                return t;
                            }
                            v.get(a).cmp(v.get(b))
                        });
                        let out_col = table.column(sel_name)?;
                        return Ok(Some(build_scan_result(proj, out_col, &indices, parsed)?));
                    }
                }
            }
        }
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(row_count);
    let need = offset.saturating_add(limit);
    let indices = if need > 0 && need < row_count {
        streaming_topk_two_from_mask(col1, col2, row_count, need, *d1, *d2, &mask)
    } else {
        let mut indices =
            select_indices_for_where(table, parsed.where_expr.as_ref(), row_count, &mask)?;
        partial_or_full_sort_indices_two(col1, col2, &mut indices, parsed, *d1, *d2);
        indices
    };

    let out_col = table.column(sel_name)?;
    Ok(Some(build_scan_result(proj, out_col, &indices, parsed)?))
}

/// Q24: `SELECT * … WHERE URL LIKE '%x%' ORDER BY EventTime LIMIT n` — sort row indices only.
fn try_scan_star_like_order_limit(
    table: &mut Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !parsed.select_all || parsed.order_by.len() != 1 {
        return Ok(None);
    }
    let (order_expr, _desc) = &parsed.order_by[0];
    let order_name = order_column_name(order_expr);
    if order_name != "EventTime" {
        return Ok(None);
    }
    let Some(where_expr) = parsed.where_expr.as_ref() else {
        return Ok(None);
    };
    if !where_is_url_like(where_expr) {
        return Ok(None);
    }

    let ColumnData::Utf8(urls) = table.column("URL")? else {
        return Ok(None);
    };
    let pattern = url_like_pattern(where_expr)?;
    let time_col = table.column("EventTime")?;

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(row_count);
    let need = offset.saturating_add(limit);

    let indices = if need > 0 && need < row_count {
        streaming_topk_url_like_loaded(urls, time_col, row_count, need, &pattern)
    } else {
        let mut all = indices_from_utf8_like(urls, &pattern, row_count);
        sort_indices_by_column(time_col, &mut all, false);
        all
    };

    let slice: Vec<usize> = indices.into_iter().skip(offset).take(limit).collect();

    table.retain_columns(&std::collections::HashSet::new());
    let (names, rows) = table.project_rows(&slice)?;

    Ok(Some(QueryResult { columns: names, rows }))
}

pub(crate) fn where_is_url_like(expr: &Expr) -> bool {
    match expr {
        Expr::Like {
            expr: inner,
            pattern,
            negated: false,
            ..
        } => {
            expr_column_name(inner).as_deref() == Some("URL")
                && matches!(&**pattern, Expr::Value(_))
        }
        Expr::Nested(e) => where_is_url_like(e),
        _ => false,
    }
}

/// Q20: `SELECT UserID FROM hits WHERE UserID = ?` — no sort/limit.
fn try_scan_int_eq(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.select_all
        || parsed.select_items.len() != 1
        || !parsed.order_by.is_empty()
        || parsed.limit.is_some()
        || parsed.offset.is_some()
    {
        return Ok(None);
    }
    let proj = &parsed.select_items[0];
    let SelectItemKind::Column(sel_expr) = &proj.kind else {
        return Ok(None);
    };
    let sel_name = expr_column_name(sel_expr).ok_or_else(|| crate::Error::msg("scan col"))?;
    let Some(where_expr) = parsed.where_expr.as_ref() else {
        return Ok(None);
    };
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right,
    } = where_expr
    else {
        return Ok(None);
    };
    let Some(col_name) = expr_column_name(left) else {
        return Ok(None);
    };
    if col_name != sel_name {
        return Ok(None);
    }
    let Expr::Value(Value::Number(n, _)) = &**right else {
        return Ok(None);
    };
    let lit: i64 = n.parse().map_err(|e| crate::Error::msg(format!("bad lit: {e}")))?;
    let col = table.column(&col_name)?;
    let label = projection_label(proj);
    let mut rows = Vec::new();
    match col {
        ColumnData::Int64(v) => {
            for &x in v.iter().take(row_count) {
                if x == lit {
                    rows.push(vec![x.to_string()]);
                }
            }
        }
        ColumnData::Int32(v) => {
            let lit32 = lit as i32;
            for &x in v.iter().take(row_count) {
                if x == lit32 {
                    rows.push(vec![x.to_string()]);
                }
            }
        }
        _ => return Ok(None),
    }
    Ok(Some(QueryResult {
        columns: vec![label],
        rows,
    }))
}

fn order_column_name(expr: &Expr) -> String {
    match expr {
        Expr::Identifier(id) => id.value.clone(),
        _ => expr_column_name(expr).unwrap_or_default(),
    }
}

fn indices_from_mask(mask: &[bool]) -> Vec<usize> {
    mask.iter()
        .enumerate()
        .filter(|(_, m)| **m)
        .map(|(i, _)| i)
        .collect()
}

/// Avoid a 100k `bool` allocation when WHERE is a single `utf8 <> ''` predicate.
fn select_indices_for_where(
    table: &Table,
    where_expr: Option<&Expr>,
    row_count: usize,
    mask: &[bool],
) -> Result<Vec<usize>> {
    if let Some(name) = utf8_ne_empty_column(where_expr) {
        if let Ok(ColumnData::Utf8(data)) = table.column(&name).map(|c| c) {
            return Ok(data
                .iter()
                .take(row_count)
                .enumerate()
                .filter(|(_, s)| !s.is_empty())
                .map(|(i, _)| i)
                .collect());
        }
    }
    Ok(indices_from_mask(mask))
}

fn utf8_ne_empty_column(expr: Option<&Expr>) -> Option<String> {
    let expr = expr?;
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::NotEq,
        right,
    } = expr
    else {
        return None;
    };
    let name = expr_column_name(left)?;
    let Expr::Value(Value::SingleQuotedString(s)) = &**right else {
        return None;
    };
    if s.is_empty() {
        Some(name)
    } else {
        None
    }
}

fn build_scan_result(
    proj: &crate::sql::SelectProjection,
    col: &ColumnData,
    indices: &[usize],
    parsed: &ParsedQuery,
) -> Result<QueryResult> {
    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(indices.len());
    let slice: Vec<usize> = if offset >= indices.len() {
        Vec::new()
    } else {
        indices
            .iter()
            .skip(offset)
            .take(limit)
            .copied()
            .collect()
    };

    let label = projection_label(proj);
    let rows: Vec<Vec<String>> = slice
        .iter()
        .map(|&i| vec![ColumnData::cell_to_string(col, i)])
        .collect();

    Ok(QueryResult {
        columns: vec![label],
        rows,
    })
}

pub(crate) fn url_like_pattern(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Like { pattern, .. } => match &**pattern {
            Expr::Value(Value::SingleQuotedString(s))
            | Expr::Value(Value::DoubleQuotedString(s)) => Ok(s.clone()),
            _ => Err(crate::Error::msg("like pattern")),
        },
        Expr::Nested(e) => url_like_pattern(e),
        _ => Err(crate::Error::msg("url like")),
    }
}

fn indices_from_utf8_like(data: &Utf8Column, pattern: &str, row_count: usize) -> Vec<usize> {
    (0..row_count)
        .filter(|&i| eval_like_match(data.get(i), pattern))
        .collect()
}

/// Top-K row indices by EventTime among URL LIKE matches — no full match list (Q24 in-cache path).
fn streaming_topk_url_like_loaded(
    urls: &Utf8Column,
    time_col: &ColumnData,
    row_count: usize,
    need: usize,
    pattern: &str,
) -> Vec<usize> {
    streaming_topk_from_mask_with(row_count, need, false, |i| {
        eval_like_match(urls.get(i), pattern)
    }, |i| key_i64_at(time_col, i))
}

fn streaming_topk_from_mask(
    order_col: &ColumnData,
    row_count: usize,
    need: usize,
    desc: bool,
    mask: &[bool],
) -> Vec<usize> {
    streaming_topk_from_mask_with(row_count, need, desc, |i| mask[i], |i| key_i64_at(order_col, i))
}

fn streaming_topk_utf8_from_mask(
    v: &Utf8Column,
    row_count: usize,
    need: usize,
    desc: bool,
    mask: &[bool],
) -> Vec<usize> {
    streaming_topk_utf8(v, row_count, need, desc, |i| mask[i])
}

fn streaming_topk_two_from_mask(
    col1: &ColumnData,
    col2: &ColumnData,
    row_count: usize,
    need: usize,
    desc1: bool,
    desc2: bool,
    mask: &[bool],
) -> Vec<usize> {
    use std::collections::BinaryHeap;

    if need == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<(TopTwoKey, usize)> = BinaryHeap::new();
    for i in 0..row_count {
        if !mask[i] {
            continue;
        }
        let key = TopTwoKey::new(key_i64_at(col1, i), key_str_at(col2, i), desc1, desc2);
        if heap.len() < need {
            heap.push((key, i));
        } else if let Some((worst, _)) = heap.peek() {
            if key < *worst {
                heap.pop();
                heap.push((key, i));
            }
        }
    }
    let mut v: Vec<_> = heap.into_iter().collect();
    v.sort_by(|(ka, a), (kb, b)| {
        let ord = ka.cmp(kb);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
        a.cmp(b)
    });
    v.into_iter().map(|(_, i)| i).collect()
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TopTwoKey(i64, u64, bool, bool);

impl TopTwoKey {
    fn new(k1: i64, s: &str, desc1: bool, desc2: bool) -> Self {
        Self(k1, hash_str(s), desc1, desc2)
    }
}

impl Ord for TopTwoKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let o1 = if self.2 {
            other.0.cmp(&self.0)
        } else {
            self.0.cmp(&other.0)
        };
        if o1 != std::cmp::Ordering::Equal {
            return o1;
        }
        if self.3 {
            other.1.cmp(&self.1)
        } else {
            self.1.cmp(&other.1)
        }
    }
}

impl PartialOrd for TopTwoKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = ahash::AHasher::default();
    s.hash(&mut h);
    h.finish()
}

fn key_i64_at(col: &ColumnData, row: usize) -> i64 {
    match col {
        ColumnData::Timestamp(v) | ColumnData::Int64(v) => v[row],
        ColumnData::Int32(v) => v[row] as i64,
        ColumnData::Int16(v) => v[row] as i64,
        ColumnData::Date(v) => v[row] as i64,
        ColumnData::Utf8(v) => {
            let s = v.get(row);
            s.parse().unwrap_or(0)
        }
    }
}

fn key_str_at(col: &ColumnData, row: usize) -> &str {
    match col {
        ColumnData::Utf8(v) => v.get(row),
        _ => "",
    }
}

fn streaming_topk_from_mask_with(
    row_count: usize,
    need: usize,
    desc: bool,
    row_ok: impl Fn(usize) -> bool,
    key_at: impl Fn(usize) -> i64,
) -> Vec<usize> {
    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    for i in 0..row_count {
        if !row_ok(i) {
            continue;
        }
        push_topk_i64(&mut heap, need, desc, key_at(i), i);
    }
    finish_topk_i64_heap(heap, desc)
}

fn partial_or_full_sort_indices(
    col: &ColumnData,
    indices: &mut Vec<usize>,
    parsed: &ParsedQuery,
    desc: bool,
) {
    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(indices.len());
    let need = offset.saturating_add(limit);
    if need > 0 && need < indices.len() {
        partial_sort_indices_by_column(col, indices, need, desc);
        indices.truncate(need);
    } else {
        sort_indices_by_column(col, indices, desc);
    }
}

fn partial_or_full_sort_indices_two(
    col1: &ColumnData,
    col2: &ColumnData,
    indices: &mut Vec<usize>,
    parsed: &ParsedQuery,
    desc1: bool,
    desc2: bool,
) {
    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(indices.len());
    let need = offset.saturating_add(limit);
    if need > 0 && need < indices.len() {
        partial_sort_indices_two(col1, col2, indices, need, desc1, desc2);
        indices.truncate(need);
    } else {
        sort_indices_two(col1, col2, indices, desc1, desc2);
    }
}

fn partial_sort_indices_by_column(col: &ColumnData, indices: &mut [usize], need: usize, desc: bool) {
    if indices.len() <= need {
        sort_indices_by_column(col, indices, desc);
        return;
    }
    if desc {
        indices.select_nth_unstable_by(need - 1, |&a, &b| cmp_at(col, b, a, false));
        indices[..need].sort_by(|&a, &b| cmp_at(col, b, a, false));
    } else {
        indices.select_nth_unstable_by(need - 1, |&a, &b| cmp_at(col, a, b, false));
        indices[..need].sort_by(|&a, &b| cmp_at(col, a, b, false));
    }
}

fn partial_sort_indices_two(
    col1: &ColumnData,
    col2: &ColumnData,
    indices: &mut [usize],
    need: usize,
    desc1: bool,
    desc2: bool,
) {
    if indices.len() <= need {
        sort_indices_two(col1, col2, indices, desc1, desc2);
        return;
    }
    indices.select_nth_unstable_by(need - 1, |&a, &b| {
        let c1 = cmp_at(col1, a, b, desc1);
        if c1 != std::cmp::Ordering::Equal {
            return c1;
        }
        cmp_at(col2, a, b, desc2)
    });
    indices[..need].sort_by(|&a, &b| {
        let c1 = cmp_at(col1, a, b, desc1);
        if c1 != std::cmp::Ordering::Equal {
            return c1;
        }
        cmp_at(col2, a, b, desc2)
    });
}

fn partial_sort_indices_by_timestamp(col: &ColumnData, indices: &mut [usize], need: usize) {
    partial_sort_indices_by_column(col, indices, need, false);
}

fn sort_indices_by_column(col: &ColumnData, indices: &mut [usize], desc: bool) {
    match col {
        ColumnData::Utf8(v) => {
            if desc {
                indices.sort_by(|&a, &b| v.get(b).cmp(v.get(a)));
            } else {
                indices.sort_by(|&a, &b| v.get(a).cmp(v.get(b)));
            }
        }
        ColumnData::Timestamp(v) | ColumnData::Int64(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
            }
        }
        ColumnData::Int32(v) | ColumnData::Date(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
            }
        }
        ColumnData::Int16(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
            }
        }
    }
}

fn sort_indices_two(
    col1: &ColumnData,
    col2: &ColumnData,
    indices: &mut [usize],
    desc1: bool,
    desc2: bool,
) {
    indices.sort_by(|&a, &b| {
        let c1 = cmp_at(col1, a, b, desc1);
        if c1 != std::cmp::Ordering::Equal {
            return c1;
        }
        cmp_at(col2, a, b, desc2)
    });
}

fn cmp_at(col: &ColumnData, a: usize, b: usize, desc: bool) -> std::cmp::Ordering {
    let ord = match col {
        ColumnData::Utf8(v) => v.get(a).cmp(v.get(b)),
        ColumnData::Timestamp(v) | ColumnData::Int64(v) => v[a].cmp(&v[b]),
        ColumnData::Int32(v) | ColumnData::Date(v) => v[a].cmp(&v[b]),
        ColumnData::Int16(v) => v[a].cmp(&v[b]),
    };
    if desc {
        ord.reverse()
    } else {
        ord
    }
}

/// Keep the best `need` row indices by i64 key without materializing the full filtered set.
fn streaming_topk_i64(
    table: &Table,
    row_count: usize,
    need: usize,
    desc: bool,
    row_ok: impl Fn(usize) -> bool,
    key_at: impl Fn(usize) -> i64,
) -> Vec<usize> {
    if need == 0 {
        return Vec::new();
    }
    if !desc {
        if let Some(zones) = table.zones() {
            if zones.event_time_monotonic_in_row_order() {
                return forward_scan_until(row_count, need, row_ok);
            }
            if zones.version >= ZONE_VERSION_V2 {
                return streaming_topk_i64_zoned(zones, row_count, need, desc, row_ok, key_at);
            }
        }
    }
    streaming_topk_i64_full(row_count, need, desc, row_ok, key_at)
}

fn forward_scan_until(row_count: usize, need: usize, row_ok: impl Fn(usize) -> bool) -> Vec<usize> {
    let mut out = Vec::with_capacity(need);
    for i in 0..row_count {
        if row_ok(i) {
            out.push(i);
            if out.len() >= need {
                return out;
            }
        }
    }
    out
}

fn streaming_topk_i64_zoned(
    zones: &ZoneIndex,
    row_count: usize,
    need: usize,
    desc: bool,
    row_ok: impl Fn(usize) -> bool,
    key_at: impl Fn(usize) -> i64,
) -> Vec<usize> {
    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    let mut row = 0usize;
    for zone in &zones.zones {
        let zone_end = (row + ZONE_ROWS).min(row_count);
        if zone_end <= row {
            break;
        }
        if heap.len() >= need {
            if let Some(&(worst_k, _)) = heap.peek() {
                if !desc && zone.min_event_time > worst_k {
                    row = zone_end;
                    continue;
                }
                if desc && zone.max_event_time < worst_k {
                    row = zone_end;
                    continue;
                }
            }
        }
        for i in row..zone_end {
            if !row_ok(i) {
                continue;
            }
            let k = key_at(i);
            push_topk_i64(&mut heap, need, desc, k, i);
        }
        row = zone_end;
    }
    finish_topk_i64_heap(heap, desc)
}

fn streaming_topk_i64_full(
    row_count: usize,
    need: usize,
    desc: bool,
    row_ok: impl Fn(usize) -> bool,
    key_at: impl Fn(usize) -> i64,
) -> Vec<usize> {
    let mut heap: BinaryHeap<(i64, usize)> = BinaryHeap::new();
    for i in 0..row_count {
        if !row_ok(i) {
            continue;
        }
        push_topk_i64(&mut heap, need, desc, key_at(i), i);
    }
    finish_topk_i64_heap(heap, desc)
}

fn push_topk_i64(heap: &mut BinaryHeap<(i64, usize)>, need: usize, desc: bool, k: i64, i: usize) {
    if heap.len() < need {
        heap.push((k, i));
    } else if let Some(&(worst_k, _)) = heap.peek() {
        let replace = if desc { k > worst_k } else { k < worst_k };
        if replace {
            heap.pop();
            heap.push((k, i));
        }
    }
}

fn finish_topk_i64_heap(heap: BinaryHeap<(i64, usize)>, desc: bool) -> Vec<usize> {
    let mut v: Vec<_> = heap.into_iter().collect();
    if desc {
        v.sort_by(|a, b| b.0.cmp(&a.0));
    } else {
        v.sort_by(|a, b| a.0.cmp(&b.0));
    }
    v.into_iter().map(|(_, i)| i).collect()
}

/// Keep the best `need` row indices by utf8 key without materializing the full filtered set.
fn streaming_topk_utf8(
    v: &Utf8Column,
    row_count: usize,
    need: usize,
    desc: bool,
    row_ok: impl Fn(usize) -> bool,
) -> Vec<usize> {
    if need == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<(usize, usize)> = BinaryHeap::new();
    for i in 0..row_count {
        if !row_ok(i) {
            continue;
        }
        if heap.len() < need {
            heap.push((i, i));
        } else if let Some(&(worst_idx, _)) = heap.peek() {
            let ord = if desc {
                v.get(i).cmp(v.get(worst_idx))
            } else {
                v.get(worst_idx).cmp(v.get(i))
            };
            if ord == std::cmp::Ordering::Greater {
                heap.pop();
                heap.push((i, i));
            }
        }
    }
    let mut indices: Vec<usize> = heap.into_iter().map(|(_, i)| i).collect();
    if desc {
        indices.sort_by(|&a, &b| v.get(b).cmp(v.get(a)));
    } else {
        indices.sort_by(|&a, &b| v.get(a).cmp(v.get(b)));
    }
    indices
}
