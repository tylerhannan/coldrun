use std::collections::HashSet;

use sqlparser::ast::Expr;

use crate::sql::{expr_column_name, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::QueryResult;
use super::{build_filter_mask, projection_label};

/// Fast path for single-row global aggregates (no GROUP BY).
pub fn try_execute_global(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !parsed.group_by.is_empty() {
        return Ok(None);
    }

    if parsed.select_items.len() > 1 {
        if let Some(r) = try_execute_global_multi_distinct(table, parsed, row_count)? {
            return Ok(Some(r));
        }
        return try_execute_global_multi(table, parsed, row_count);
    }

    let mask = build_filter_mask(&table, parsed.where_expr.as_ref(), row_count)?;
    let selected = count_selected(&mask);

    let proj = &parsed.select_items[0];
    let col_name = projection_label(proj);

    match &proj.kind {
        SelectItemKind::CountAll | SelectItemKind::Count(_) => {
            return Ok(Some(QueryResult {
                columns: vec![col_name],
                rows: vec![vec![selected.to_string()]],
            }));
        }
        SelectItemKind::Sum(expr) => {
            if let Some(name) = expr_column_name(expr) {
                if let Ok(sum) = sum_column_masked(table, &name, &mask) {
                    return Ok(Some(QueryResult {
                        columns: vec![col_name],
                        rows: vec![vec![sum.to_string()]],
                    }));
                }
            }
        }
        SelectItemKind::Avg(expr) => {
            if let Some(name) = expr_column_name(expr) {
                if let Ok((sum, n)) = sum_column_masked_with_count(table, &name, &mask) {
                    if n > 0 {
                        let avg = sum as f64 / n as f64;
                        return Ok(Some(QueryResult {
                            columns: vec![col_name],
                            rows: vec![vec![format!("{avg}")]],
                        }));
                    }
                }
            }
        }
        SelectItemKind::CountDistinct(expr) => {
            if let Some(n) = count_distinct_masked(table, expr, &mask) {
                return Ok(Some(QueryResult {
                    columns: vec![col_name],
                    rows: vec![vec![n.to_string()]],
                }));
            }
        }
        SelectItemKind::Min(expr) | SelectItemKind::Max(expr) => {
            if let Some(v) =
                minmax_column_masked(table, expr, &mask, matches!(proj.kind, SelectItemKind::Max(_)))
            {
                return Ok(Some(QueryResult {
                    columns: vec![col_name],
                    rows: vec![vec![v]],
                }));
            }
        }
        _ => {}
    }
    Ok(None)
}

/// One mask pass for Q3-style `SELECT SUM(..), COUNT(*), AVG(..)` on int columns.
fn try_execute_global_multi(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let mut plans = Vec::with_capacity(parsed.select_items.len());
    for proj in &parsed.select_items {
        plans.push(match classify_simple(&proj.kind)? {
            Some(p) => p,
            None => return Ok(None),
        });
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = count_selected(&mask);

    let mut columns = Vec::with_capacity(parsed.select_items.len());
    let mut values = Vec::with_capacity(parsed.select_items.len());

    for (proj, plan) in parsed.select_items.iter().zip(plans.iter()) {
        columns.push(projection_label(proj));
        values.push(match plan {
            SimpleAgg::CountAll => selected.to_string(),
            SimpleAgg::Sum(name) => sum_column_masked(table, name, &mask)?.to_string(),
            SimpleAgg::Avg(name) => {
                let (sum, n) = sum_column_masked_with_count(table, name, &mask)?;
                let avg = sum as f64 / n.max(1) as f64;
                format!("{avg}")
            }
            SimpleAgg::CountDistinct(name) => count_distinct_col_masked(table, name, &mask)
                .ok_or_else(|| crate::Error::msg("count distinct"))?
                .to_string(),
        });
    }

    Ok(Some(QueryResult {
        columns,
        rows: vec![values],
    }))
}

enum SimpleAgg {
    CountAll,
    Sum(String),
    Avg(String),
    CountDistinct(String),
}

/// Q5+Q6 style: two global `COUNT(DISTINCT col)` in one query (one mask pass).
fn try_execute_global_multi_distinct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.select_items.len() != 2 {
        return Ok(None);
    }
    let mut names = Vec::new();
    for proj in &parsed.select_items {
        match &proj.kind {
            SelectItemKind::CountDistinct(e) => {
                names.push(expr_column_name(e).ok_or_else(|| crate::Error::msg("col"))?);
            }
            _ => return Ok(None),
        }
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let mut columns = Vec::new();
    let mut values = Vec::new();
    for (proj, name) in parsed.select_items.iter().zip(names.iter()) {
        columns.push(projection_label(proj));
        values.push(
            count_distinct_col_masked(table, name, &mask)
                .ok_or_else(|| crate::Error::msg("count distinct"))?
                .to_string(),
        );
    }
    Ok(Some(QueryResult {
        columns,
        rows: vec![values],
    }))
}

fn classify_simple(kind: &SelectItemKind) -> Result<Option<SimpleAgg>> {
    Ok(match kind {
        SelectItemKind::CountAll | SelectItemKind::Count(_) => Some(SimpleAgg::CountAll),
        SelectItemKind::Sum(e) => expr_column_name(e).map(SimpleAgg::Sum),
        SelectItemKind::Avg(e) => expr_column_name(e).map(SimpleAgg::Avg),
        SelectItemKind::CountDistinct(e) => expr_column_name(e).map(SimpleAgg::CountDistinct),
        _ => None,
    })
}

fn count_selected(mask: &[bool]) -> u64 {
    mask.iter().map(|&b| u64::from(b)).sum()
}

fn count_distinct_masked(table: &Table, expr: &Expr, mask: &[bool]) -> Option<u64> {
    let name = expr_column_name(expr)?;
    count_distinct_col_masked(table, &name, mask)
}

fn count_distinct_col_masked(table: &Table, name: &str, mask: &[bool]) -> Option<u64> {
    let col = table.column(name).ok()?;
    match col {
        ColumnData::Int64(v) => {
            let mut set = HashSet::new();
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    set.insert(x);
                }
            }
            Some(set.len() as u64)
        }
        ColumnData::Int32(v) => {
            let mut set = HashSet::new();
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    set.insert(i64::from(x));
                }
            }
            Some(set.len() as u64)
        }
        ColumnData::Int16(v) => {
            let mut set = HashSet::new();
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    set.insert(i64::from(x));
                }
            }
            Some(set.len() as u64)
        }
        ColumnData::Utf8(v) => {
            let mut set = HashSet::<String>::new();
            for (i, s) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    set.insert(s.clone());
                }
            }
            Some(set.len() as u64)
        }
        _ => None,
    }
}

fn sum_column_masked(table: &Table, name: &str, mask: &[bool]) -> Result<i128> {
    let (sum, _) = sum_column_masked_with_count(table, name, mask)?;
    Ok(sum)
}

fn sum_column_masked_with_count(table: &Table, name: &str, mask: &[bool]) -> Result<(i128, u64)> {
    let col = table.column(name)?;
    let mut sum = 0i128;
    let mut n = 0u64;
    match col {
        ColumnData::Int64(v) => {
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    sum += x as i128;
                    n += 1;
                }
            }
        }
        ColumnData::Int32(v) => {
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    sum += i64::from(x) as i128;
                    n += 1;
                }
            }
        }
        ColumnData::Int16(v) => {
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    sum += i64::from(x) as i128;
                    n += 1;
                }
            }
        }
        _ => return Err(crate::Error::msg("sum on non-int column")),
    }
    Ok((sum, n))
}

fn minmax_column_masked(table: &Table, expr: &Expr, mask: &[bool], is_max: bool) -> Option<String> {
    let name = expr_column_name(expr)?;
    let col = table.column(&name).ok()?;
    match col {
        ColumnData::Date(v) => {
            let mut opt: Option<i32> = None;
            for (i, &x) in v.iter().enumerate() {
                if !mask.get(i).copied().unwrap_or(false) {
                    continue;
                }
                opt = Some(match opt {
                    None => x,
                    Some(cur) if is_max => cur.max(x),
                    Some(cur) => cur.min(x),
                });
            }
            return opt.map(|d| d.to_string());
        }
        ColumnData::Int64(v) => {
            let mut opt: Option<i64> = None;
            for (i, &x) in v.iter().enumerate() {
                if !mask.get(i).copied().unwrap_or(false) {
                    continue;
                }
                opt = Some(match opt {
                    None => x,
                    Some(cur) if is_max => cur.max(x),
                    Some(cur) => cur.min(x),
                });
            }
            return opt.map(|d| d.to_string());
        }
        _ => {}
    }
    None
}
