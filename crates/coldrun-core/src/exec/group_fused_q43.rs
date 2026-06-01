//! Q43: DATE_TRUNC minute EventTime + COUNT, ordered slice with OFFSET.

use sqlparser::ast::{Expr, FunctionArg};

use crate::expr::format_timestamp_micros_trunc;
use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::group::resolve_group_expr;
use super::group_fused::build_mask;
use super::mask_util::for_each_selected;
use super::QueryResult;

pub fn try_fused_q43(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !is_q43_shape(parsed) {
        return Ok(None);
    }
    let ColumnData::Timestamp(times) = table.column("EventTime")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(Some(QueryResult {
            columns: parsed.select_items.iter().map(projection_label).collect(),
            rows: vec![],
        }));
    };

    let mut counts: ahash::AHashMap<i64, u64> = ahash::AHashMap::with_capacity(512);
    for_each_selected(&mask, row_count, |i| {
        let bucket = minute_bucket_micros(times[i]);
        *counts.entry(bucket).or_insert(0) += 1;
    });

    let mut keys: Vec<i64> = counts.keys().copied().collect();
    keys.sort_unstable();

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(10);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let rows: Vec<Vec<String>> = keys
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|bucket| {
            vec![
                format_timestamp_micros_trunc(bucket),
                counts[&bucket].to_string(),
            ]
        })
        .collect();

    Ok(Some(QueryResult { columns, rows }))
}

fn minute_bucket_micros(micros: i64) -> i64 {
    let secs = micros / 1_000_000;
    (secs - secs.rem_euclid(60)) * 1_000_000
}

fn is_q43_shape(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 1 || parsed.having.is_some() {
        return false;
    }
    if !is_date_trunc_minute_event_time(&resolve_group_expr(
        &parsed.group_by[0],
        &parsed.select_items,
    )) {
        return false;
    }
    parsed.select_items.len() == 2
        && matches!(
            parsed.select_items[1].kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_)
        )
}

fn is_date_trunc_minute_event_time(expr: &Expr) -> bool {
    let Expr::Function(f) = expr else {
        return false;
    };
    if f.name.to_string().to_uppercase() != "DATE_TRUNC" {
        return false;
    }
    let args = match &f.args {
        sqlparser::ast::FunctionArguments::List(l) => &l.args,
        _ => return false,
    };
    if args.len() < 2 {
        return false;
    }
    let unit_ok = match args.first() {
        Some(FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(Expr::Identifier(id)))) => {
            id.value.eq_ignore_ascii_case("minute")
        }
        Some(FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(Expr::Value(
            sqlparser::ast::Value::SingleQuotedString(s),
        )))) => s.eq_ignore_ascii_case("minute"),
        _ => false,
    };
    let col_ok = matches!(
        args.get(1),
        Some(sqlparser::ast::FunctionArg::Unnamed(
            sqlparser::ast::FunctionArgExpr::Expr(Expr::Identifier(id))
        )) if id.value == "EventTime"
    );
    unit_ok && col_ok
}
