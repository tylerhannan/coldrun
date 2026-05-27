//! Fast path for ClickBench Q29: many `SUM(ResolutionWidth + k)` in one SELECT.

use rayon::prelude::*;
use sqlparser::ast::Expr;

use crate::sql::{ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::QueryResult;
use super::projection_label;

pub fn try_execute_q29(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !is_q29_shape(parsed) {
        return Ok(None);
    }

    let col = table.column("ResolutionWidth")?;
    let ColumnData::Int16(widths) = col else {
        return Ok(None);
    };

    let n = row_count.min(widths.len());
    let ks: Vec<i16> = parsed
        .select_items
        .iter()
        .map(|proj| match &proj.kind {
            SelectItemKind::Sum(expr) => {
                if matches!(expr, Expr::Identifier(id) if id.value == "ResolutionWidth") {
                    Ok(0i16)
                } else {
                    parse_resolution_plus_k(expr)
                }
            }
            _ => Err(crate::Error::msg("expected sum")),
        })
        .collect::<Result<_>>()?;

    let sums: Vec<i128> = if n > 50_000 {
        widths
            .par_iter()
            .take(n)
            .fold(
                || vec![0i128; ks.len()],
                |mut acc, &w| {
                    for (sum, &k) in acc.iter_mut().zip(ks.iter()) {
                        *sum += i64::from(w.saturating_add(k)) as i128;
                    }
                    acc
                },
            )
            .reduce(
                || vec![0i128; ks.len()],
                |mut a, b| {
                    for (x, y) in a.iter_mut().zip(b) {
                        *x += y;
                    }
                    a
                },
            )
    } else {
        let mut acc = vec![0i128; ks.len()];
        for &w in widths.iter().take(n) {
            for (sum, &k) in acc.iter_mut().zip(ks.iter()) {
                *sum += i64::from(w.saturating_add(k)) as i128;
            }
        }
        acc
    };

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let values: Vec<String> = sums.iter().map(|s| s.to_string()).collect();

    Ok(Some(QueryResult {
        columns,
        rows: vec![values],
    }))
}

fn is_q29_shape(parsed: &ParsedQuery) -> bool {
    if !parsed.group_by.is_empty()
        || parsed.where_expr.is_some()
        || parsed.select_items.len() < 10
    {
        return false;
    }
    parsed
        .select_items
        .iter()
        .all(|p| matches!(p.kind, SelectItemKind::Sum(_)))
}

fn parse_resolution_plus_k(expr: &Expr) -> Result<i16> {
    use sqlparser::ast::{BinaryOperator, Value};
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Plus,
        right,
    } = expr
    else {
        return Err(crate::Error::msg("expected plus"));
    };
    let k = match (left.as_ref(), right.as_ref()) {
        (Expr::Identifier(id), Expr::Value(Value::Number(n, _))) if id.value == "ResolutionWidth" => {
            n.parse::<i16>()
                .map_err(|_| crate::Error::msg("bad k"))?
        }
        (Expr::Value(Value::Number(n, _)), Expr::Identifier(id))
            if id.value == "ResolutionWidth" =>
        {
            n.parse::<i16>()
                .map_err(|_| crate::Error::msg("bad k"))?
        }
        _ => return Err(crate::Error::msg("expected ResolutionWidth + k")),
    };
    Ok(k)
}
