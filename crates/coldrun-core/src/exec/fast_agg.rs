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
    let mask = build_filter_mask(&table, parsed.where_expr.as_ref(), row_count)?;
    let selected = count_selected(&mask);

    if parsed.select_items.len() != 1 {
        return Ok(None);
    }
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
        SelectItemKind::Min(expr) | SelectItemKind::Max(expr) => {
            if let Some(v) = minmax_column_masked(table, expr, &mask, matches!(proj.kind, SelectItemKind::Max(_))) {
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

fn count_selected(mask: &[bool]) -> u64 {
    mask.iter().filter(|&&b| b).count() as u64
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
