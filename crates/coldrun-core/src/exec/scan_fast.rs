//! Fast scan for `SELECT col … ORDER BY col LIMIT` (Q25–Q27 pattern).

use sqlparser::ast::Expr;

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
    if parsed.select_all || parsed.select_items.len() != 1 {
        return Ok(None);
    }
    if parsed.order_by.len() != 1 {
        return Ok(None);
    }

    let proj = &parsed.select_items[0];
    let SelectItemKind::Column(sel_expr) = &proj.kind else {
        return Ok(None);
    };
    let sel_name = expr_column_name(sel_expr).ok_or_else(|| crate::Error::msg("scan col"))?;

    let (order_expr, desc) = &parsed.order_by[0];
    let order_name = match order_expr {
        Expr::Identifier(id) => id.value.clone(),
        _ => expr_column_name(order_expr).unwrap_or_default(),
    };
    if order_name != sel_name {
        return Ok(None);
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let col = table.column(&sel_name)?;
    let mut indices: Vec<usize> = mask
        .iter()
        .enumerate()
        .filter(|(_, m)| **m)
        .map(|(i, _)| i)
        .collect();

    sort_indices_by_column(col, &mut indices, *desc);

    let offset = parsed.offset.unwrap_or(0) as usize;
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(indices.len());
    if offset >= indices.len() {
        indices.clear();
    } else {
        indices = indices.into_iter().skip(offset).take(limit).collect();
    }

    let label = projection_label(proj);
    let rows: Vec<Vec<String>> = indices
        .iter()
        .map(|&i| vec![col_key(col, i)])
        .collect();

    Ok(Some(QueryResult {
        columns: vec![label],
        rows,
    }))
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
        ColumnData::Timestamp(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
            }
        }
        ColumnData::Int64(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
            }
        }
        ColumnData::Int32(v) => {
            if desc {
                indices.sort_by(|&a, &b| v[b].cmp(&v[a]));
            } else {
                indices.sort_by(|&a, &b| v[a].cmp(&v[b]));
            }
        }
        ColumnData::Date(v) => {
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
