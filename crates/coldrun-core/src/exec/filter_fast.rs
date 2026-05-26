//! Vectorized filter masks for common ClickBench predicate shapes.

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::expr::{eval_bool, eval_like_match, parse_date_lit};
use crate::sql::expr_column_name;
use crate::storage::{ColumnData, Table};
use crate::Result;

pub fn build_filter_mask(
    table: &Table,
    where_expr: Option<&Expr>,
    row_count: usize,
) -> Result<Vec<bool>> {
    let Some(expr) = where_expr else {
        return Ok(vec![true; row_count]);
    };
    if let Some(mut mask) = try_build_mask(table, expr, row_count)? {
        try_zone_prune(table, expr, &mut mask);
        return Ok(mask);
    }
    let mut mask = Vec::with_capacity(row_count);
    for i in 0..row_count {
        mask.push(eval_bool(table, expr, i)?);
    }
    try_zone_prune(table, expr, &mut mask);
    Ok(mask)
}

/// If WHERE is CounterID = N plus EventDate range, clear mask bits for non-matching PK zones.
fn try_zone_prune(table: &Table, expr: &Expr, mask: &mut [bool]) {
    let Some(zones) = table.zones() else {
        return;
    };
    let Some((counter, min_date, max_date)) = extract_counter_date_range(expr) else {
        return;
    };
    zones.apply_dashboard_prune(mask, counter, min_date, max_date);
}

fn extract_counter_date_range(expr: &Expr) -> Option<(i32, i32, i32)> {
    let mut counter = None;
    let mut min_date = None;
    let mut max_date = None;
    collect_counter_date(expr, &mut counter, &mut min_date, &mut max_date);
    Some((counter?, min_date?, max_date?))
}

fn collect_counter_date(
    expr: &Expr,
    counter: &mut Option<i32>,
    min_date: &mut Option<i32>,
    max_date: &mut Option<i32>,
) {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            collect_counter_date(left, counter, min_date, max_date);
            collect_counter_date(right, counter, min_date, max_date);
        }
        Expr::BinaryOp {
            left,
            op,
            right,
        } => {
            if let (Some(name), Expr::Value(rv)) = (expr_column_name(left), &**right) {
                if name == "CounterID" && matches!(op, BinaryOperator::Eq) {
                    if let Ok(v) = value_as_i64(rv) {
                        *counter = Some(v as i32);
                    }
                }
                if name == "EventDate" && is_date_lit(rv) {
                    if let Ok(s) = date_str(rv) {
                        if let Ok(d) = parse_date_lit(s) {
                            match op {
                                BinaryOperator::GtEq => *min_date = Some(d),
                                BinaryOperator::LtEq => *max_date = Some(d),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        Expr::Nested(inner) => collect_counter_date(inner, counter, min_date, max_date),
        _ => {}
    }
}

fn try_build_mask(table: &Table, expr: &Expr, row_count: usize) -> Result<Option<Vec<bool>>> {
    match expr {
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::And => {
                let Some(l) = try_build_mask(table, left, row_count)? else {
                    return Ok(None);
                };
                let Some(r) = try_build_mask(table, right, row_count)? else {
                    return Ok(None);
                };
                Ok(Some(and_masks(&l, &r)))
            }
            BinaryOperator::Or => {
                let Some(l) = try_build_mask(table, left, row_count)? else {
                    return Ok(None);
                };
                let Some(r) = try_build_mask(table, right, row_count)? else {
                    return Ok(None);
                };
                Ok(Some(or_masks(&l, &r)))
            }
            _ => try_cmp_mask(table, left, op, right, row_count),
        },
        Expr::Like {
            negated,
            expr: inner,
            pattern,
            ..
        } => try_like_mask(table, inner, pattern, *negated, row_count),
        Expr::Nested(inner) => try_build_mask(table, inner, row_count),
        _ => Ok(None),
    }
}

fn try_cmp_mask(
    table: &Table,
    left: &Expr,
    op: &BinaryOperator,
    right: &Expr,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    if let (Some(name), Expr::Value(rv)) = (expr_column_name(left), right) {
        let col = table.column(&name)?;
        if is_date_lit(rv) {
            let lit = parse_date_lit(date_str(rv)?)?;
            return Ok(Some(cmp_date_col(col, lit, op, row_count)));
        }
        if let Ok(lit) = value_as_i64(rv) {
            return Ok(Some(cmp_int_col(col, lit, op, row_count)));
        }
        if is_string_lit(rv) {
            let lit = string_lit(rv)?;
            if matches!(op, BinaryOperator::NotEq | BinaryOperator::Eq) {
                return Ok(Some(cmp_utf8_col(col, &lit, op, row_count)));
            }
        }
    }
    Ok(None)
}

fn try_like_mask(
    table: &Table,
    inner: &Expr,
    pattern: &Expr,
    negated: bool,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    let Some(name) = expr_column_name(inner) else {
        return Ok(None);
    };
    let Ok(col) = table.column(&name) else {
        return Ok(None);
    };
    let ColumnData::Utf8(data) = col else {
        return Ok(None);
    };
    let Expr::Value(v) = pattern else {
        return Ok(None);
    };
    let Ok(pat) = string_lit(v) else {
        return Ok(None);
    };
    let mut mask = Vec::with_capacity(row_count);
    for s in data.iter().take(row_count) {
        let m = eval_like_match(s, &pat);
        mask.push(if negated { !m } else { m });
    }
    mask.resize(row_count, false);
    Ok(Some(mask))
}

fn and_masks(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b).map(|(x, y)| *x && *y).collect()
}

fn or_masks(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b).map(|(x, y)| *x || *y).collect()
}

fn cmp_int_col(col: &ColumnData, lit: i64, op: &BinaryOperator, row_count: usize) -> Vec<bool> {
    let mut mask = Vec::with_capacity(row_count);
    match col {
        ColumnData::Int64(v) => {
            for &x in v.iter().take(row_count) {
                mask.push(cmp_i64(x, lit, op));
            }
        }
        ColumnData::Int32(v) => {
            let lit = lit as i32;
            for &x in v.iter().take(row_count) {
                mask.push(cmp_i64(i64::from(x), i64::from(lit), op));
            }
        }
        ColumnData::Int16(v) => {
            let lit = lit as i16;
            for &x in v.iter().take(row_count) {
                mask.push(cmp_i64(i64::from(x), i64::from(lit), op));
            }
        }
        _ => mask.resize(row_count, false),
    }
    mask.resize(row_count, false);
    mask
}

fn cmp_date_col(col: &ColumnData, lit: i32, op: &BinaryOperator, row_count: usize) -> Vec<bool> {
    let mut mask = Vec::with_capacity(row_count);
    if let ColumnData::Date(v) = col {
        let lit64 = i64::from(lit);
        for &x in v.iter().take(row_count) {
            mask.push(cmp_i64(i64::from(x), lit64, op));
        }
    }
    mask.resize(row_count, false);
    mask
}

fn cmp_utf8_col(col: &ColumnData, lit: &str, op: &BinaryOperator, row_count: usize) -> Vec<bool> {
    let mut mask = Vec::with_capacity(row_count);
    if let ColumnData::Utf8(v) = col {
        for s in v.iter().take(row_count) {
            mask.push(match op {
                BinaryOperator::NotEq if lit.is_empty() => !s.is_empty(),
                BinaryOperator::Eq if lit.is_empty() => s.is_empty(),
                BinaryOperator::NotEq => s != lit,
                BinaryOperator::Eq => s == lit,
                _ => false,
            });
        }
    }
    mask.resize(row_count, false);
    mask
}

fn cmp_i64(v: i64, lit: i64, op: &BinaryOperator) -> bool {
    match op {
        BinaryOperator::Eq => v == lit,
        BinaryOperator::NotEq => v != lit,
        BinaryOperator::Gt => v > lit,
        BinaryOperator::GtEq => v >= lit,
        BinaryOperator::Lt => v < lit,
        BinaryOperator::LtEq => v <= lit,
        _ => false,
    }
}

fn is_date_lit(v: &Value) -> bool {
    matches!(
        v,
        Value::SingleQuotedString(s) if s.len() == 10 && s.as_bytes().get(4) == Some(&b'-')
    )
}

fn is_string_lit(v: &Value) -> bool {
    matches!(
        v,
        Value::SingleQuotedString(_)
            | Value::DoubleQuotedString(_)
            | Value::EscapedStringLiteral(_)
    )
}

fn date_str(v: &Value) -> Result<&str> {
    match v {
        Value::SingleQuotedString(s) => Ok(s.as_str()),
        _ => Err(crate::Error::msg("expected date string")),
    }
}

fn string_lit(v: &Value) -> Result<String> {
    Ok(match v {
        Value::SingleQuotedString(s)
        | Value::DoubleQuotedString(s)
        | Value::EscapedStringLiteral(s) => s.clone(),
        _ => return Err(crate::Error::msg("expected string")),
    })
}

fn value_as_i64(v: &Value) -> Result<i64> {
    match v {
        Value::Number(n, _) => n
            .parse()
            .map_err(|_| crate::Error::msg(format!("bad number {n}"))),
        _ => Err(crate::Error::msg("expected number")),
    }
}
