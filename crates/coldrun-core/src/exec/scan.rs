use sqlparser::ast::Expr;

use crate::expr::{eval_i64, eval_string};
use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::Database;
use crate::Result;

use super::filter::build_filter_mask;
use super::QueryResult;

pub fn execute_scan(db: &Database, parsed: &ParsedQuery) -> Result<QueryResult> {
    let table = db.open_table_for_query("hits", parsed)?;
    let row_count = table.row_count() as usize;
    let mask = build_filter_mask(&table, parsed.where_expr.as_ref(), row_count)?;

    let column_names: Vec<String> = if parsed.select_all {
        table.column_names().map(|s| s.to_string()).collect()
    } else {
        parsed.select_items.iter().map(projection_label).collect()
    };

    let mut rows: Vec<(Vec<String>, Vec<String>)> = Vec::new();

    for i in 0..row_count {
        if !mask[i] {
            continue;
        }
        let values = if parsed.select_all {
            table
                .column_names()
                .map(|name| cell_at(&table, name, i))
                .collect::<Result<Vec<_>>>()?
        } else {
            parsed
                .select_items
                .iter()
                .map(|p| eval_projection(&table, p, i))
                .collect::<Result<Vec<_>>>()?
        };
        let sort_key = sort_keys(&table, &parsed.order_by, i)?;
        rows.push((sort_key, values));
    }

    if !parsed.order_by.is_empty() {
        rows.sort_by(|a, b| {
            for ((ka, kb), (_, desc)) in a.0.iter().zip(b.0.iter()).zip(parsed.order_by.iter()) {
                let cmp = compare_cell(ka, kb);
                let ord = if *desc {
                    cmp.reverse()
                } else {
                    cmp
                };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    if let Some(offset) = parsed.offset {
        let off = offset as usize;
        if off < rows.len() {
            rows.drain(0..off);
        } else {
            rows.clear();
        }
    }

    if let Some(limit) = parsed.limit {
        rows.truncate(limit as usize);
    }

    let out_rows: Vec<Vec<String>> = rows.into_iter().map(|(_, v)| v).collect();

    Ok(QueryResult {
        columns: column_names,
        rows: out_rows,
    })
}

fn eval_projection(
    table: &crate::storage::Table,
    proj: &crate::sql::SelectProjection,
    row: usize,
) -> Result<String> {
    match &proj.kind {
        SelectItemKind::Column(e) | SelectItemKind::Other(e) => {
            if let Ok(s) = eval_string(table, e, row) {
                return Ok(s);
            }
            Ok(eval_i64(table, e, row)?.to_string())
        }
        _ => Err(crate::Error::msg("scan projection must be column expr")),
    }
}

fn cell_at(table: &crate::storage::Table, name: &str, row: usize) -> Result<String> {
    let col = table.column(name)?;
    Ok(crate::exec::col_key(col, row))
}

fn sort_keys(
    table: &crate::storage::Table,
    order_by: &[(Expr, bool)],
    row: usize,
) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    for (expr, _) in order_by {
        if let Ok(s) = eval_string(table, expr, row) {
            keys.push(s);
        } else {
            keys.push(eval_i64(table, expr, row)?.to_string());
        }
    }
    Ok(keys)
}

fn compare_cell(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}
