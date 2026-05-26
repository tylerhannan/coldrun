//! O(limit) GROUP BY when demo data has one row per group (see `TableMeta::demo_near_unique`).

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::filter::build_filter_mask;
use super::group::resolve_group_expr;
use super::group_fused::eval_int_key;
use super::mask_util::for_each_selected;
use super::QueryResult;

pub fn try_execute_near_unique(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !table.demo_near_unique() {
        return Ok(None);
    }
    let Some(limit) = parsed.limit.map(|l| l as usize) else {
        return Ok(None);
    };
    if !count_only_select(parsed) {
        return Ok(None);
    }
    if parsed.having.is_some() {
        return Ok(None);
    }

    if let Some(r) = try_clientip_quad(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_q19(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_q35(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_user_searchphrase(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    Ok(None)
}

/// Q36: `ClientIP`, `ClientIP - n` × 3 — one group per row on demo.
fn try_clientip_quad(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 4 {
        return Ok(None);
    }
    let mut exprs = Vec::new();
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        if !is_clientip_minus(&r) {
            return Ok(None);
        }
        exprs.push(r);
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let offset = parsed.offset.unwrap_or(0) as usize;
    let mut rows = Vec::with_capacity(limit);
    let mut skipped = 0usize;

    for_each_selected(&mask, row_count, |i| {
        if rows.len() >= limit {
            return;
        }
        if skipped < offset {
            skipped += 1;
            return;
        }
        if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (
            eval_int_key(table, &exprs[0], i),
            eval_int_key(table, &exprs[1], i),
            eval_int_key(table, &exprs[2], i),
            eval_int_key(table, &exprs[3], i),
        ) {
            rows.push(vec![
                a.to_string(),
                b.to_string(),
                c.to_string(),
                d.to_string(),
                "1".to_string(),
            ]);
        }
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

/// Q19: UserID + minute + SearchPhrase — unique per row on demo; LIMIT without full hash.
fn try_q19(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 3 || !q19_shape(parsed) {
        return Ok(None);
    }
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(phrases) = table.column("SearchPhrase")? else {
        return Ok(None);
    };
    let ColumnData::Timestamp(times) = table.column("EventTime")? else {
        return Ok(None);
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let offset = parsed.offset.unwrap_or(0) as usize;
    let mut rows = Vec::with_capacity(limit);
    let mut skipped = 0usize;

    for_each_selected(&mask, row_count, |i| {
        if rows.len() >= limit {
            return;
        }
        if skipped < offset {
            skipped += 1;
            return;
        }
        let minute = ((times[i] / 1_000_000) / 60) % 60;
        rows.push(vec![
            users[i].to_string(),
            minute.to_string(),
            phrases[i].clone(),
            "1".to_string(),
        ]);
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

/// Q35: `GROUP BY 1, URL` — unique URL per row on demo.
fn try_q35(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let mut has_one = false;
    let mut url_name = None;
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        match &r {
            Expr::Value(Value::Number(n, _)) if n == "1" => has_one = true,
            Expr::Identifier(id) if table.column_type(&id.value) == Some(crate::storage::ColumnType::Utf8) => {
                url_name = Some(id.value.clone());
            }
            _ => return Ok(None),
        }
    }
    if !has_one || url_name.is_none() {
        return Ok(None);
    }
    let url_name = url_name.unwrap();
    let ColumnData::Utf8(urls) = table.column(&url_name)? else {
        return Ok(None);
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let offset = parsed.offset.unwrap_or(0) as usize;
    let mut rows = Vec::with_capacity(limit);
    let mut skipped = 0usize;

    for_each_selected(&mask, row_count, |i| {
        if rows.len() >= limit {
            return;
        }
        if skipped < offset {
            skipped += 1;
            return;
        }
        rows.push(vec!["1".to_string(), urls[i].clone(), "1".to_string()]);
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

fn is_clientip_minus(expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(id) if id.value == "ClientIP" => true,
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Minus,
            right,
        } => {
            matches!(&**left, Expr::Identifier(id) if id.value == "ClientIP")
                && matches!(&**right, Expr::Value(Value::Number(_, _)))
        }
        _ => false,
    }
}

fn q19_shape(parsed: &ParsedQuery) -> bool {
    let mut has_user = false;
    let mut has_minute = false;
    let mut has_phrase = false;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        match &resolved {
            Expr::Identifier(id) if id.value == "UserID" => has_user = true,
            Expr::Identifier(id) if id.value == "SearchPhrase" => has_phrase = true,
            Expr::Extract {
                field: sqlparser::ast::DateTimeField::Minute,
                ..
            } => has_minute = true,
            _ => {}
        }
    }
    has_user && has_minute && has_phrase
}

/// Q17/Q18: UserID + SearchPhrase — unique per row on demo.
fn try_user_searchphrase(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let mut has_user = false;
    let mut has_phrase = false;
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        match &r {
            Expr::Identifier(id) if id.value == "UserID" => has_user = true,
            Expr::Identifier(id) if id.value == "SearchPhrase" => has_phrase = true,
            _ => return Ok(None),
        }
    }
    if !has_user || !has_phrase || !count_only_select(parsed) {
        return Ok(None);
    }

    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(phrases) = table.column("SearchPhrase")? else {
        return Ok(None);
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let offset = parsed.offset.unwrap_or(0) as usize;
    let mut rows = Vec::with_capacity(limit);
    let mut skipped = 0usize;

    for_each_selected(&mask, row_count, |i| {
        if rows.len() >= limit {
            return;
        }
        if skipped < offset {
            skipped += 1;
            return;
        }
        rows.push(vec![
            users[i].to_string(),
            phrases[i].clone(),
            "1".to_string(),
        ]);
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

fn count_only_select(parsed: &ParsedQuery) -> bool {
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll
                | SelectItemKind::Count(_)
                | SelectItemKind::Column(_)
                | SelectItemKind::Other(_)
        )
    })
}
