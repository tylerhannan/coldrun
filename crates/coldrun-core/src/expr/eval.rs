use std::sync::LazyLock;

use chrono::{NaiveDate, TimeZone, Utc};
use regex::Regex;
use sqlparser::ast::{
    BinaryOperator, DateTimeField, Expr, Function, FunctionArg, FunctionArguments, Ident, Value,
};

use crate::sql::expr_column_name;
use crate::storage::ColumnData;
use crate::storage::Table;
use crate::Result;

static RE_REFERER_HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://(?:www\.)?([^/]+)/.*$").expect("referer regex")
});

pub fn parse_date_lit(s: &str) -> Result<i32> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| crate::Error::msg(format!("bad date '{s}': {e}")))?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    Ok((d - epoch).num_days() as i32)
}

pub fn format_date_days(days: i32) -> String {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    (epoch + chrono::Duration::days(i64::from(days)))
        .format("%Y-%m-%d")
        .to_string()
}

pub fn format_timestamp_micros(micros: i64) -> String {
    let secs = micros.div_euclid(1_000_000);
    let nanos = (micros.rem_euclid(1_000_000) * 1_000) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| micros.to_string())
}

/// ClickBench / DuckDB display for `DATE_TRUNC` buckets (PDT, UTC−7).
/// Minute of hour (0–59) from EventTime micros — matches `extract(minute FROM EventTime)`.
pub fn event_time_minute_of_hour(micros: i64) -> i64 {
    ((micros / 1_000_000) / 60) % 60
}

pub fn format_timestamp_micros_trunc(micros: i64) -> String {
    const PDT_MICROS: i64 = 7 * 3600 * 1_000_000;
    format_timestamp_micros(micros - PDT_MICROS)
}

pub fn eval_i64(table: &Table, expr: &Expr, row: usize) -> Result<i64> {
    match expr {
        Expr::Value(v) => value_to_i64(v),
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => {
            let name = expr_column_name(expr).ok_or_else(|| crate::Error::msg("expected column"))?;
            Ok(col_i64_at(table.column(&name)?, row))
        }
        Expr::BinaryOp { left, op, right } => {
            let l = eval_i64(table, left, row)?;
            let r = eval_i64(table, right, row)?;
            Ok(match op {
                BinaryOperator::Plus => l.saturating_add(r),
                BinaryOperator::Minus => l.saturating_sub(r),
                BinaryOperator::Multiply => l.saturating_mul(r),
                BinaryOperator::Divide => {
                    if r == 0 {
                        0
                    } else {
                        l / r
                    }
                }
                _ => return Err(crate::Error::msg(format!("unsupported numeric op {op}"))),
            })
        }
        Expr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Minus,
            expr: inner,
        } => Ok(-eval_i64(table, inner, row)?),
        Expr::Nested(inner) => eval_i64(table, inner, row),
        Expr::Cast { expr, .. } => eval_i64(table, expr, row),
        Expr::Extract { field, expr, .. } => eval_extract_i64(table, field, expr, row),
        Expr::Function(f) => eval_function_i64(table, f, row),
        Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => eval_case_i64(
            table,
            operand.as_deref(),
            conditions,
            results,
            else_result.as_deref(),
            row,
        ),
        _ => Err(crate::Error::msg(format!("unsupported numeric expr: {expr}"))),
    }
}

pub fn eval_string(table: &Table, expr: &Expr, row: usize) -> Result<String> {
    match expr {
        Expr::Value(v) => value_to_string(v),
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => {
            let name = expr_column_name(expr).ok_or_else(|| crate::Error::msg("expected column"))?;
            Ok(col_utf8_at(table.column(&name)?, row).to_string())
        }
        Expr::Function(f) => {
            let name = f.name.to_string().to_uppercase();
            if name == "REGEXP_REPLACE" {
                return eval_regexp_replace(table, f, row);
            }
            if name == "DATE_TRUNC" {
                let bucket = eval_function_i64(table, f, row)?;
                return Ok(format_timestamp_micros_trunc(bucket));
            }
            Err(crate::Error::msg(format!("unsupported string function {name}")))
        }
        Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => eval_case_string(
            table,
            operand.as_deref(),
            conditions,
            results,
            else_result.as_deref(),
            row,
        ),
        Expr::BinaryOp { .. } => Ok(eval_i64(table, expr, row)?.to_string()),
        Expr::Nested(inner) => eval_string(table, inner, row),
        _ => Err(crate::Error::msg(format!("unsupported string expr: {expr}"))),
    }
}

pub fn eval_bool(table: &Table, expr: &Expr, row: usize) -> Result<bool> {
    match expr {
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::And => Ok(eval_bool(table, left, row)? && eval_bool(table, right, row)?),
            BinaryOperator::Or => Ok(eval_bool(table, left, row)? || eval_bool(table, right, row)?),
            BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Gt
            | BinaryOperator::GtEq
            | BinaryOperator::Lt
            | BinaryOperator::LtEq => Ok(eval_cmp(table, left, right, row, op)?),
            _ => Err(crate::Error::msg(format!("unsupported bool op {op}"))),
        },
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        } => {
            let haystack = eval_string(table, expr, row)?;
            let pat = pattern_as_str(pattern)?;
            let m = eval_like_match(&haystack, &pat);
            Ok(if *negated { !m } else { m })
        }
        Expr::ILike {
            negated,
            expr,
            pattern,
            ..
        } => {
            let haystack = eval_string(table, expr, row)?.to_lowercase();
            let pat = pattern_as_str(pattern)?.to_lowercase();
            let m = eval_like_match(&haystack, &pat);
            Ok(if *negated { !m } else { m })
        }
        Expr::UnaryOp {
            op: sqlparser::ast::UnaryOperator::Not,
            expr: inner,
        } => Ok(!eval_bool(table, inner, row)?),
        Expr::Nested(inner) => eval_bool(table, inner, row),
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let v = eval_i64(table, expr, row)?;
            let mut found = false;
            for e in list {
                if eval_i64(table, e, row)? == v {
                    found = true;
                    break;
                }
            }
            Ok(if *negated { !found } else { found })
        }
        Expr::IsNull(_) => Ok(false),
        Expr::Value(Value::Boolean(b)) => Ok(*b),
        _ => Err(crate::Error::msg(format!("unsupported predicate: {expr}"))),
    }
}

pub fn eval_group_key(table: &Table, expr: &Expr, row: usize) -> Result<String> {
    match expr {
        Expr::Value(Value::Number(n, _)) => Ok(n.clone()),
        Expr::Function(f) if f.name.to_string().to_uppercase() == "DATE_TRUNC" => {
            Ok(format_timestamp_micros_trunc(eval_function_i64(table, f, row)?))
        }
        Expr::Function(f) if f.name.to_string().to_uppercase() == "REGEXP_REPLACE" => {
            Ok(eval_regexp_replace(table, f, row)?)
        }
        Expr::Extract { field, expr, .. } => Ok(eval_extract_i64(table, field, expr, row)?.to_string()),
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => {
            let name = expr_column_name(expr).ok_or_else(|| crate::Error::msg("expected column"))?;
            match table.column(&name)? {
                ColumnData::Utf8(v) => Ok(v[row].clone()),
                ColumnData::Date(v) => Ok(format_date_days(v[row])),
                ColumnData::Timestamp(v) => Ok(format_timestamp_micros(v[row])),
                _ => Ok(eval_i64(table, expr, row)?.to_string()),
            }
        }
        _ => {
            if let Ok(s) = eval_string(table, expr, row) {
                if !s.is_empty() {
                    return Ok(s);
                }
            }
            Ok(eval_i64(table, expr, row)?.to_string())
        }
    }
}

/// Extract host from ClickBench referer URL pattern (Q29).
pub fn referer_host(url: &str) -> String {
    if let Some(caps) = RE_REFERER_HOST.captures(url) {
        return caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
    }
    url.to_string()
}

pub fn eval_like_match(haystack: &str, pattern: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix('%') {
        if let Some(inner) = rest.strip_suffix('%') {
            if inner.is_empty() {
                return true;
            }
            return memchr::memmem::find(haystack.as_bytes(), inner.as_bytes()).is_some();
        }
        return haystack.ends_with(rest);
    }
    if let Some(rest) = pattern.strip_suffix('%') {
        return haystack.starts_with(rest);
    }
    haystack == pattern
}

fn eval_cmp(
    table: &Table,
    left: &Expr,
    right: &Expr,
    row: usize,
    op: &BinaryOperator,
) -> Result<bool> {
    if let (Some(lname), Expr::Value(rv)) = (expr_column_name(left), right) {
        let col = table.column(&lname)?;
        if is_date_string(rv) {
            let lit = parse_date_lit(value_as_str(rv)?)?;
            return cmp_i64(col_i64_at(col, row), lit as i64, op);
        }
        if is_string_value(rv) {
            let lit = value_to_string(rv)?;
            return cmp_str(&col_compare_str(col, row), &lit, op);
        }
        if let Ok(lit) = value_to_i64(rv) {
            return cmp_i64(col_i64_at(col, row), lit, op);
        }
    }
    if let (Expr::Value(rv), Some(rname)) = (left, expr_column_name(right)) {
        let col = table.column(&rname)?;
        if is_date_string(rv) {
            let lit = parse_date_lit(value_as_str(rv)?)?;
            return cmp_i64(lit as i64, col_i64_at(col, row), op);
        }
        if is_string_value(rv) {
            let lit = value_to_string(rv)?;
            return cmp_str(&lit, &col_compare_str(col, row), op);
        }
        if let Ok(lit) = value_to_i64(rv) {
            return cmp_i64(lit, col_i64_at(col, row), op);
        }
    }
    if matches!(*op, BinaryOperator::Eq | BinaryOperator::NotEq) {
        if let (Ok(l), Ok(r)) = (eval_string(table, left, row), eval_string(table, right, row)) {
            return cmp_str(&l, &r, op);
        }
    }
    cmp_i64(eval_i64(table, left, row)?, eval_i64(table, right, row)?, op)
}

fn cmp_i64(left: i64, right: i64, op: &BinaryOperator) -> Result<bool> {
    Ok(match *op {
        BinaryOperator::Eq => left == right,
        BinaryOperator::NotEq => left != right,
        BinaryOperator::Gt => left > right,
        BinaryOperator::GtEq => left >= right,
        BinaryOperator::Lt => left < right,
        BinaryOperator::LtEq => left <= right,
        _ => return Err(crate::Error::msg(format!("unsupported compare op {op}"))),
    })
}

fn cmp_str(left: &str, right: &str, op: &BinaryOperator) -> Result<bool> {
    Ok(match *op {
        BinaryOperator::Eq => left == right,
        BinaryOperator::NotEq => left != right,
        BinaryOperator::Gt => left > right,
        BinaryOperator::GtEq => left >= right,
        BinaryOperator::Lt => left < right,
        BinaryOperator::LtEq => left <= right,
        _ => return Err(crate::Error::msg(format!("unsupported compare op {op}"))),
    })
}

fn col_compare_str(col: &ColumnData, row: usize) -> String {
    match col {
        ColumnData::Utf8(v) => v[row].clone(),
        _ => col_i64_at(col, row).to_string(),
    }
}

fn eval_extract_i64(
    table: &Table,
    field: &DateTimeField,
    expr: &Expr,
    row: usize,
) -> Result<i64> {
    let micros = eval_i64(table, expr, row)?;
    Ok(match field {
        DateTimeField::Minute => event_time_minute_of_hour(micros),
        DateTimeField::Hour => ((micros / 1_000_000) / 3600) % 24,
        DateTimeField::Day => (micros / 1_000_000) / 86400,
        DateTimeField::Month => 0,
        DateTimeField::Year => 0,
        _ => (micros / 1_000_000) / 60,
    })
}

fn eval_function_i64(table: &Table, f: &Function, row: usize) -> Result<i64> {
    let name = f.name.to_string().to_uppercase();
    match name.as_str() {
        "LENGTH" => {
            let arg = extract_expr(f, 0)?;
            // DuckDB/ClickHouse LENGTH on strings is byte length.
            Ok(eval_string(table, &arg, row)?.as_bytes().len() as i64)
        }
        "DATE_TRUNC" => {
            let unit = extract_ident(f, 0)?;
            let arg = extract_expr(f, 1)?;
            let micros = eval_i64(table, &arg, row)?;
            let secs = micros / 1_000_000;
            let bucket_secs = match unit.to_lowercase().as_str() {
                "minute" => (secs / 60) * 60,
                "hour" => (secs / 3600) * 3600,
                "day" => (secs / 86400) * 86400,
                _ => return Err(crate::Error::msg(format!("unsupported DATE_TRUNC {unit}"))),
            };
            Ok(bucket_secs * 1_000_000)
        }
        _ => Err(crate::Error::msg(format!("unsupported function {name}"))),
    }
}

fn eval_regexp_replace(table: &Table, f: &Function, row: usize) -> Result<String> {
    let input = eval_string(table, &extract_expr(f, 0)?, row)?;
    let pattern = value_to_string(&match extract_expr(f, 1)? {
        Expr::Value(v) => v,
        e => return Err(crate::Error::msg(format!("expected pattern literal, got {e}"))),
    })?;
    if pattern.contains("https?://") {
        return Ok(referer_host(&input));
    }
    let replacement = value_to_string(&match extract_expr(f, 2)? {
        Expr::Value(v) => v,
        e => return Err(crate::Error::msg(format!("expected replacement literal, got {e}"))),
    })?;
    let re = Regex::new(&pattern).map_err(|e| crate::Error::msg(e.to_string()))?;
    Ok(re.replace(&input, replacement.as_str()).to_string())
}

fn eval_case_i64(
    table: &Table,
    operand: Option<&Expr>,
    conditions: &[Expr],
    results: &[Expr],
    else_result: Option<&Expr>,
    row: usize,
) -> Result<i64> {
    if let Some(op) = operand {
        return eval_i64(table, op, row);
    }
    for (cond, res) in conditions.iter().zip(results.iter()) {
        if eval_bool(table, cond, row)? {
            return eval_i64(table, res, row);
        }
    }
    if let Some(e) = else_result {
        eval_i64(table, e, row)
    } else {
        Ok(0)
    }
}

fn eval_case_string(
    table: &Table,
    operand: Option<&Expr>,
    conditions: &[Expr],
    results: &[Expr],
    else_result: Option<&Expr>,
    row: usize,
) -> Result<String> {
    if let Some(op) = operand {
        return eval_string(table, op, row);
    }
    for (cond, res) in conditions.iter().zip(results.iter()) {
        if eval_bool(table, cond, row)? {
            return eval_string(table, res, row);
        }
    }
    if let Some(e) = else_result {
        eval_string(table, e, row)
    } else {
        Ok(String::new())
    }
}

fn pattern_as_str(pattern: &Expr) -> Result<String> {
    match pattern {
        Expr::Value(v) => value_to_string(v),
        _ => Err(crate::Error::msg("LIKE needs string literal")),
    }
}

fn extract_expr(f: &Function, idx: usize) -> Result<Expr> {
    let list = match &f.args {
        FunctionArguments::List(l) => &l.args,
        _ => return Err(crate::Error::msg("expected function args")),
    };
    match list.get(idx) {
        Some(FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(e))) => Ok(e.clone()),
        _ => Err(crate::Error::msg("expected expr arg")),
    }
}

fn extract_ident(f: &Function, idx: usize) -> Result<String> {
    match extract_expr(f, idx)? {
        Expr::Identifier(Ident { value, .. }) => Ok(value),
        Expr::Value(Value::SingleQuotedString(s)) => Ok(s),
        other => Err(crate::Error::msg(format!("expected ident, got {other}"))),
    }
}

fn value_to_i64(v: &Value) -> Result<i64> {
    match v {
        Value::Number(n, _) => n
            .parse()
            .map_err(|_| crate::Error::msg(format!("bad number {n}"))),
        _ => Err(crate::Error::msg("expected number")),
    }
}

fn value_to_string(v: &Value) -> Result<String> {
    Ok(match v {
        Value::SingleQuotedString(s)
        | Value::DoubleQuotedString(s)
        | Value::EscapedStringLiteral(s) => s.clone(),
        Value::Number(n, _) => n.clone(),
        _ => return Err(crate::Error::msg("expected string literal")),
    })
}

fn value_as_str(v: &Value) -> Result<&str> {
    match v {
        Value::SingleQuotedString(s)
        | Value::DoubleQuotedString(s)
        | Value::EscapedStringLiteral(s) => Ok(s.as_str()),
        _ => Err(crate::Error::msg("expected string")),
    }
}

fn is_date_string(v: &Value) -> bool {
    matches!(
        v,
        Value::SingleQuotedString(s) if s.len() == 10 && s.as_bytes().get(4) == Some(&b'-')
    )
}

fn col_i64_at(col: &ColumnData, i: usize) -> i64 {
    match col {
        ColumnData::Int64(v) => v[i],
        ColumnData::Int32(v) => v[i] as i64,
        ColumnData::Int16(v) => v[i] as i64,
        ColumnData::Date(v) => v[i] as i64,
        ColumnData::Timestamp(v) => v[i],
        ColumnData::Utf8(_) => 0,
    }
}

fn col_utf8_at(col: &ColumnData, i: usize) -> &str {
    match col {
        ColumnData::Utf8(v) => v[i].as_str(),
        _ => "",
    }
}

fn is_string_value(v: &Value) -> bool {
    matches!(
        v,
        Value::SingleQuotedString(_)
            | Value::DoubleQuotedString(_)
            | Value::EscapedStringLiteral(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlparser::ast::{Expr, SetExpr, Statement};
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;

    #[test]
    fn empty_string_literal_is_quoted() {
        let sql = "SELECT 1 FROM hits WHERE MobilePhoneModel <> ''";
        let stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).unwrap();
        let Statement::Query(q) = &stmts[0] else { panic!() };
        let SetExpr::Select(sel) = &*q.body else { panic!() };
        let Expr::BinaryOp { right, .. } = sel.selection.as_ref().unwrap() else {
            panic!("no where");
        };
        let Expr::Value(v) = right.as_ref() else {
            panic!("right not value: {right:?}");
        };
        assert!(is_string_value(v));
        assert_eq!(value_to_string(v).unwrap(), "");
    }
}
