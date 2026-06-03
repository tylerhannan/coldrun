//! Vectorized filter masks for common ClickBench predicate shapes.

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::expr::{eval_bool, eval_like_match, parse_date_lit};
use crate::sql::expr_column_name;
use crate::storage::{ColumnData, Table, Utf8Column};
use crate::Result;

pub fn build_filter_mask(
    table: &Table,
    where_expr: Option<&Expr>,
    row_count: usize,
) -> Result<Vec<bool>> {
    let Some(expr) = where_expr else {
        return Ok(vec![true; row_count]);
    };
    if let Some(mask) = try_build_mask_dashboard_sparse(table, expr, row_count)? {
        return Ok(mask);
    }
    if let Some(mut mask) = try_build_mask_and_fused(table, expr, row_count)? {
        try_zone_prune(table, expr, &mut mask);
        try_adv_zone_prune(table, expr, &mut mask);
        return Ok(mask);
    }
    if let Some(mut mask) = try_build_mask(table, expr, row_count)? {
        try_zone_prune(table, expr, &mut mask);
        try_adv_zone_prune(table, expr, &mut mask);
        return Ok(mask);
    }
    let mut mask = Vec::with_capacity(row_count);
    for i in 0..row_count {
        mask.push(eval_bool(table, expr, i)?);
    }
    try_zone_prune(table, expr, &mut mask);
    try_adv_zone_prune(table, expr, &mut mask);
    Ok(mask)
}

/// AND tree with dashboard PK predicates: start sparse (false) instead of dense all-true.
fn try_build_mask_dashboard_sparse(
    table: &Table,
    expr: &Expr,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    let Some(zones) = table.zones() else {
        return Ok(None);
    };
    let parts = flatten_and(expr);
    let dash = parts
        .iter()
        .find_map(|p| extract_counter_date_range(p));
    let Some((counter, min_date, max_date)) = dash else {
        return Ok(None);
    };
    let mut mask = zones.build_sparse_dashboard_mask(row_count, counter, min_date, max_date);
    for part in parts {
        if extract_counter_date_range(part).is_some() {
            continue;
        }
        let Some(sub) = try_build_mask(table, part, row_count)? else {
            return Ok(None);
        };
        and_masks_inplace(&mut mask, &sub);
    }
    try_adv_zone_prune(table, expr, &mut mask);
    Ok(Some(mask))
}

fn flatten_and<'a>(expr: &'a Expr) -> Vec<&'a Expr> {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let mut v = flatten_and(left);
            v.extend(flatten_and(right));
            v
        }
        Expr::Nested(inner) => flatten_and(inner),
        other => vec![other],
    }
}

enum FusedPred<'a> {
    Utf8NeEmpty(&'a Utf8Column),
    Utf8Like {
        data: &'a Utf8Column,
        pattern: String,
        negated: bool,
    },
    IntNeZeroInt16(&'a [i16]),
    IntNeZeroInt32(&'a [i32]),
    IntNeZeroInt64(&'a [i64]),
}

impl FusedPred<'_> {
    fn eval(&self, row: usize) -> bool {
        match self {
            FusedPred::Utf8NeEmpty(v) => !v.get(row).is_empty(),
            FusedPred::Utf8Like {
                data,
                pattern,
                negated,
            } => {
                let m = eval_like_match(data.get(row), pattern);
                if *negated {
                    !m
                } else {
                    m
                }
            }
            FusedPred::IntNeZeroInt16(v) => v[row] != 0,
            FusedPred::IntNeZeroInt32(v) => v[row] != 0,
            FusedPred::IntNeZeroInt64(v) => v[row] != 0,
        }
    }
}

fn try_build_mask_and_fused(
    table: &Table,
    expr: &Expr,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    let parts = flatten_and(expr);
    if parts.len() < 2 {
        return Ok(None);
    }
    if parts.iter().any(|p| extract_counter_date_range(p).is_some()) {
        return Ok(None);
    }
    let mut preds = Vec::with_capacity(parts.len());
    for part in parts {
        let pred = match compile_fused_pred(table, part) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };
        preds.push(pred);
    }
    let mut mask = Vec::with_capacity(row_count);
    for i in 0..row_count {
        mask.push(preds.iter().all(|p| p.eval(i)));
    }
    Ok(Some(mask))
}

fn compile_fused_pred<'a>(table: &'a Table, expr: &'a Expr) -> Result<FusedPred<'a>> {
    if let Expr::Like {
        negated,
        expr: inner,
        pattern,
        ..
    } = expr
    {
        let name = expr_column_name(inner).ok_or_else(|| crate::Error::msg("like col"))?;
        let col = table.column(&name)?;
        let ColumnData::Utf8(data) = col else {
            return Err(crate::Error::msg("like on non-utf8"));
        };
        let Expr::Value(v) = &**pattern else {
            return Err(crate::Error::msg("like pattern"));
        };
        return Ok(FusedPred::Utf8Like {
            data,
            pattern: string_lit(v)?,
            negated: *negated,
        });
    }
    if let Expr::BinaryOp {
        left,
        op: BinaryOperator::NotEq,
        right,
    } = expr
    {
        let name = expr_column_name(left).ok_or_else(|| crate::Error::msg("neq col"))?;
        if let Expr::Value(v) = &**right {
            if let Ok(s) = string_lit(v) {
                if s.is_empty() {
                    let col = table.column(&name)?;
                    return Ok(match col {
                        ColumnData::Utf8(data) => FusedPred::Utf8NeEmpty(data),
                        _ => return Err(crate::Error::msg("neq empty on non-utf8")),
                    });
                }
            }
            if let Ok(0) = value_as_i64(v) {
                let col = table.column(&name)?;
                return Ok(match col {
                    ColumnData::Int16(v) => FusedPred::IntNeZeroInt16(v),
                    ColumnData::Int32(v) => FusedPred::IntNeZeroInt32(v),
                    ColumnData::Int64(v) => FusedPred::IntNeZeroInt64(v),
                    _ => return Err(crate::Error::msg("neq zero on non-int")),
                });
            }
        }
    }
    Err(crate::Error::msg("unsupported fused pred"))
}

fn and_masks_inplace(a: &mut [bool], b: &[bool]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x &= *y;
    }
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

fn try_adv_zone_prune(table: &Table, expr: &Expr, mask: &mut [bool]) {
    if !is_adv_ne_zero(expr) {
        return;
    }
    let Some(zones) = table.zones() else {
        return;
    };
    zones.apply_adv_ne_zero_prune(mask);
}

fn is_adv_ne_zero(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::NotEq,
            right,
        } => {
            expr_column_name(left).as_deref() == Some("AdvEngineID")
                && matches!(&**right, Expr::Value(Value::Number(n, _)) if n == "0")
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => is_adv_ne_zero(left) || is_adv_ne_zero(right),
        Expr::Nested(inner) => is_adv_ne_zero(inner),
        _ => false,
    }
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
        Expr::InList {
            expr,
            list,
            negated,
        } => try_in_list_mask(table, expr, list, *negated, row_count),
        Expr::Nested(inner) => try_build_mask(table, inner, row_count),
        _ => Ok(None),
    }
}

fn try_in_list_mask(
    table: &Table,
    expr: &Expr,
    list: &[Expr],
    negated: bool,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    let Some(name) = expr_column_name(expr) else {
        return Ok(None);
    };
    let Ok(col) = table.column(&name) else {
        return Ok(None);
    };
    let mut lits = Vec::new();
    for e in list {
        let Expr::Value(v) = e else {
            return Ok(None);
        };
        lits.push(value_as_i64(v)?);
    }
    let mut mask: Vec<bool> = match col {
        ColumnData::Int16(v) => {
            let set: ahash::AHashSet<i16> = lits.iter().map(|&n| n as i16).collect();
            v.iter()
                .take(row_count)
                .map(|&x| set.contains(&x) ^ negated)
                .collect()
        }
        ColumnData::Int32(v) => {
            let set: ahash::AHashSet<i32> = lits.iter().map(|&n| n as i32).collect();
            v.iter()
                .take(row_count)
                .map(|&x| set.contains(&x) ^ negated)
                .collect()
        }
        ColumnData::Int64(v) => {
            let set: ahash::AHashSet<i64> = lits.iter().copied().collect();
            v.iter()
                .take(row_count)
                .map(|&x| set.contains(&x) ^ negated)
                .collect()
        }
        _ => return Ok(None),
    };
    mask.resize(row_count, false);
    Ok(Some(mask))
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
    let mut out = a.to_vec();
    for (x, y) in out.iter_mut().zip(b) {
        *x &= *y;
    }
    out
}

fn or_masks(a: &[bool], b: &[bool]) -> Vec<bool> {
    let mut out = a.to_vec();
    for (x, y) in out.iter_mut().zip(b) {
        *x |= *y;
    }
    out
}

fn cmp_int_col(col: &ColumnData, lit: i64, op: &BinaryOperator, row_count: usize) -> Vec<bool> {
    if matches!(op, BinaryOperator::NotEq) && lit == 0 {
        return cmp_int_ne_zero(col, row_count);
    }
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

fn cmp_int_ne_zero(col: &ColumnData, row_count: usize) -> Vec<bool> {
    let mut mask = Vec::with_capacity(row_count);
    match col {
        ColumnData::Int64(v) => {
            for &x in v.iter().take(row_count) {
                mask.push(x != 0);
            }
        }
        ColumnData::Int32(v) => {
            for &x in v.iter().take(row_count) {
                mask.push(x != 0);
            }
        }
        ColumnData::Int16(v) => {
            for &x in v.iter().take(row_count) {
                mask.push(x != 0);
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
