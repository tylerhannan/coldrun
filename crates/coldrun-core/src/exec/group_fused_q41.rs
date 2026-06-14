//! Q41: dashboard filter + URLHash/EventDate GROUP BY — zone scan + sharded top-K.

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::expr::parse_date_lit;
use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::column_slice;
use super::filter::build_filter_mask;
use super::group_columnar::{dashboard_q41_topk, mask_selected_pair_topk};
use super::group_fused::{group_id_name, orders_by_count_desc, unpack_pair_keys};
use super::QueryResult;

struct Q41Pred {
    counter: i32,
    min_date: i32,
    max_date: i32,
    is_refresh: i16,
    referer_hash: i64,
}

pub fn try_fused_q41(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 || !orders_by_count_desc(parsed) {
        return Ok(None);
    }
    let k1 = match group_id_name(&parsed.group_by[0], parsed) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let k2 = match group_id_name(&parsed.group_by[1], parsed) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    if !((k1 == "URLHash" && k2 == "EventDate") || (k1 == "EventDate" && k2 == "URLHash")) {
        return Ok(None);
    }
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return Ok(None);
    }
    let Some(pred) = parse_q41_where(parsed.where_expr.as_ref())? else {
        return Ok(None);
    };

    let c1 = table.column(&k1)?;
    let c2 = table.column(&k2)?;
    let ic1 = column_slice::as_int_cols(c1).ok_or_else(|| crate::Error::msg("k1"))?;
    let ic2 = column_slice::as_int_cols(c2).ok_or_else(|| crate::Error::msg("k2"))?;
    let ColumnData::Int32(counters) = table.column("CounterID")? else {
        return Ok(None);
    };
    let ColumnData::Date(dates) = table.column("EventDate")? else {
        return Ok(None);
    };
    let ColumnData::Int16(refresh) = table.column("IsRefresh")? else {
        return Ok(None);
    };
    let ColumnData::Int16(traffic) = table.column("TraficSourceID")? else {
        return Ok(None);
    };
    let ColumnData::Int64(referer) = table.column("RefererHash")? else {
        return Ok(None);
    };

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(10);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let url_high = k1 == "URLHash";
    let ColumnData::Int64(url_hashes) = table.column("URLHash")? else {
        return Ok(None);
    };

    let entries = if let Some(zones) = table.zones() {
        let ranges = zones.dashboard_matching_ranges(
            row_count,
            pred.counter,
            pred.min_date,
            pred.max_date,
        );
        if ranges.is_empty() {
            vec![]
        } else {
            dashboard_q41_topk(
                &ranges,
                row_count,
                pred.referer_hash,
                pred.counter,
                pred.min_date,
                pred.max_date,
                pred.is_refresh,
                referer,
                counters,
                dates,
                refresh,
                traffic,
                url_hashes,
                url_high,
                limit,
                offset,
            )
        }
    } else {
        let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
        mask_selected_pair_topk(&mask, row_count, ic1, ic2, limit, offset)
    };

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let rows: Vec<Vec<String>> = entries
        .into_iter()
        .map(|(key, c)| {
            let (a, bb) = unpack_pair_keys(c1, c2, key);
            vec![a, bb, c.to_string()]
        })
        .collect();

    Ok(Some(QueryResult { columns, rows }))
}

fn parse_q41_where(expr: Option<&Expr>) -> Result<Option<Q41Pred>> {
    let Some(expr) = expr else {
        return Ok(None);
    };
    let parts = flatten_and(expr);
    let mut counter = None;
    let mut min_date = None;
    let mut max_date = None;
    let mut is_refresh = None;
    let mut traffic = None;
    let mut referer = None;

    for part in parts {
        if let Some((c, min_d, max_d)) = extract_counter_date_range(part) {
            counter = Some(c);
            min_date = Some(min_d);
            max_date = Some(max_d);
            continue;
        }
        if let Some(v) = extract_eq_i16(part, "IsRefresh") {
            is_refresh = Some(v);
            continue;
        }
        if let Some(set) = extract_in_i16(part, "TraficSourceID") {
            traffic = Some(set);
            continue;
        }
        if let Some(v) = extract_eq_i64(part, "RefererHash") {
            referer = Some(v);
            continue;
        }
        return Ok(None);
    }

    let (counter, min_date, max_date, is_refresh, referer_hash) = match (
        counter,
        min_date,
        max_date,
        is_refresh,
        traffic,
        referer,
    ) {
        (Some(c), Some(min_d), Some(max_d), Some(r), Some(t), Some(ref_h)) => {
            if t.len() != 2 || !t.contains(&-1) || !t.contains(&6) {
                return Ok(None);
            }
            (c, min_d, max_d, r, ref_h)
        }
        _ => return Ok(None),
    };

    Ok(Some(Q41Pred {
        counter,
        min_date,
        max_date,
        is_refresh,
        referer_hash,
    }))
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
        Expr::BinaryOp { left, op, right } => {
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

fn extract_eq_i16(expr: &Expr, col: &str) -> Option<i16> {
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right,
    } = expr
    else {
        return None;
    };
    if expr_column_name(left)? != col {
        return None;
    }
    let Expr::Value(v) = &**right else {
        return None;
    };
    value_as_i64(v).ok().map(|n| n as i16)
}

fn extract_eq_i64(expr: &Expr, col: &str) -> Option<i64> {
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right,
    } = expr
    else {
        return None;
    };
    if expr_column_name(left)? != col {
        return None;
    }
    let Expr::Value(v) = &**right else {
        return None;
    };
    value_as_i64(v).ok()
}

fn extract_in_i16(expr: &Expr, col: &str) -> Option<ahash::AHashSet<i16>> {
    let Expr::InList {
        expr: inner,
        list,
        negated: false,
    } = expr
    else {
        return None;
    };
    if expr_column_name(inner)? != col {
        return None;
    }
    let mut set = ahash::AHashSet::new();
    for e in list {
        let Expr::Value(v) = e else {
            return None;
        };
        set.insert(value_as_i64(v).ok()? as i16);
    }
    Some(set)
}

fn is_date_lit(v: &Value) -> bool {
    matches!(v, Value::SingleQuotedString(_) | Value::DoubleQuotedString(_))
}

fn date_str(v: &Value) -> Result<&str> {
    match v {
        Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => Ok(s.as_str()),
        _ => Err(crate::Error::msg("date lit")),
    }
}

fn value_as_i64(v: &Value) -> Result<i64> {
    match v {
        Value::Number(n, _) => n
            .parse::<i64>()
            .map_err(|e| crate::Error::msg(e.to_string())),
        _ => Err(crate::Error::msg("number")),
    }
}
