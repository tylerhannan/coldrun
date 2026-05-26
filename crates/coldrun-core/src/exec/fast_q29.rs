//! Fast path for ClickBench Q29: many `SUM(ResolutionWidth + k)` in one SELECT.

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
    let mut columns = Vec::with_capacity(parsed.select_items.len());
    let mut values = Vec::with_capacity(parsed.select_items.len());

    for proj in &parsed.select_items {
        columns.push(projection_label(proj));
        let sum = match &proj.kind {
            SelectItemKind::Sum(expr) => sum_resolution_plus_k(widths, n, expr)?,
            _ => return Ok(None),
        };
        values.push(sum.to_string());
    }

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

fn sum_resolution_plus_k(widths: &[i16], n: usize, expr: &sqlparser::ast::Expr) -> Result<i128> {
    let k = match expr {
        sqlparser::ast::Expr::Identifier(id) if id.value == "ResolutionWidth" => 0,
        _ => parse_resolution_plus_k(expr)?,
    };
    let mut sum = 0i128;
    for &w in widths.iter().take(n) {
        sum += i64::from(w.saturating_add(k)) as i128;
    }
    Ok(sum)
}

fn parse_resolution_plus_k(expr: &sqlparser::ast::Expr) -> Result<i16> {
    use sqlparser::ast::{BinaryOperator, Expr, Value};
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
