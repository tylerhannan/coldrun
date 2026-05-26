//! Fast scan for `SELECT col … ORDER BY … LIMIT` (Q25–Q27 pattern).

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::sql::{expr_column_name, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::filter::build_filter_mask;
use super::QueryResult;
use super::{col_key, projection_label};

pub fn try_execute_scan_fast(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = try_scan_int_eq(table, parsed, row_count)? {
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
        1 => try_scan_single_order(table, parsed, row_count, proj, &sel_name),
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

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let col = table.column(sel_name)?;
    let mut indices = indices_from_mask(&mask);
    sort_indices_by_column(col, &mut indices, *desc);
    Ok(Some(build_scan_result(
        proj,
        col,
        &indices,
        parsed,
    )?))
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
    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let mut indices = indices_from_mask(&mask);
    sort_indices_two(col1, col2, &mut indices, *d1, *d2);

    let out_col = table.column(sel_name)?;
    Ok(Some(build_scan_result(proj, out_col, &indices, parsed)?))
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
        .map(|&i| vec![col_key(col, i)])
        .collect();

    Ok(QueryResult {
        columns: vec![label],
        rows,
    })
}

fn sort_indices_by_column(col: &ColumnData, indices: &mut [usize], desc: bool) {
    match col {
        ColumnData::Utf8(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
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
        ColumnData::Utf8(v) => v[a].cmp(&v[b]),
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
