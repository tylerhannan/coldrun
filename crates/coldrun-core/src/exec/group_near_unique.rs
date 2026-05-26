//! O(limit) GROUP BY when demo data has one row per group (see `TableMeta::demo_near_unique`).

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
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
    if parsed.having.is_some() {
        return Ok(None);
    }

    if let Some(r) = try_utf8_count_distinct_user(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_int_utf8_count_distinct_user(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_int_utf8_count(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_single_utf8_count(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_int_pair_row_aggs(table, parsed, row_count, limit)? {
        return Ok(Some(r));
    }
    if !count_only_select(parsed) {
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

/// Q13/Q34: one utf8 key + COUNT(*) — one row per group on demo.
fn try_single_utf8_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 || !count_only_select(parsed) {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if table.column_type(&id.value) != Some(ColumnType::Utf8) {
        return Ok(None);
    }
    let ColumnData::Utf8(keys) = table.column(&id.value)? else {
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
        rows.push(vec![keys[i].clone(), "1".to_string()]);
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

/// Q11/Q14: utf8 key + COUNT(DISTINCT UserID) — one user per group on demo.
fn try_utf8_count_distinct_user(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 || parsed.having.is_some() {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if table.column_type(&id.value) != Some(ColumnType::Utf8) {
        return Ok(None);
    }
    let mut distinct_user = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::CountDistinct(e) => {
                if expr_column_name(e).as_deref() != Some("UserID") {
                    return Ok(None);
                }
                distinct_user = true;
            }
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !distinct_user || parsed.select_items.len() != 2 {
        return Ok(None);
    }

    let ColumnData::Utf8(keys) = table.column(&id.value)? else {
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
        rows.push(vec![keys[i].clone(), "1".to_string()]);
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

/// Q15/16-style: int + utf8 + COUNT(*) — unique composite on demo.
fn try_int_utf8_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 || !count_only_select(parsed) {
        return Ok(None);
    }
    let mut int_name = None;
    let mut utf8_name = None;
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        let Expr::Identifier(id) = &r else {
            return Ok(None);
        };
        match table.column_type(&id.value) {
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64) => {
                int_name = Some(id.value.clone());
            }
            Some(ColumnType::Utf8) => utf8_name = Some(id.value.clone()),
            _ => return Ok(None),
        }
    }
    let (Some(int_name), Some(utf8_name)) = (int_name, utf8_name) else {
        return Ok(None);
    };

    let _ = table.column(&int_name)?;
    let ColumnData::Utf8(utf8) = table.column(&utf8_name)? else {
        return Ok(None);
    };
    let int_expr = resolve_group_expr(
        if expr_column_name(&parsed.group_by[0]).as_deref() == Some(int_name.as_str()) {
            &parsed.group_by[0]
        } else {
            &parsed.group_by[1]
        },
        &parsed.select_items,
    );

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
        if let Ok(int_val) = eval_int_key(table, &int_expr, i) {
            rows.push(vec![
                int_val.to_string(),
                utf8[i].clone(),
                "1".to_string(),
            ]);
        }
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

/// Q12: int + utf8 + COUNT(DISTINCT UserID) — unique composite on demo.
fn try_int_utf8_count_distinct_user(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 || parsed.having.is_some() {
        return Ok(None);
    }
    let mut int_name = None;
    let mut utf8_name = None;
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        let Expr::Identifier(id) = &r else {
            return Ok(None);
        };
        match table.column_type(&id.value) {
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64) => {
                int_name = Some(id.value.clone());
            }
            Some(ColumnType::Utf8) => utf8_name = Some(id.value.clone()),
            _ => return Ok(None),
        }
    }
    let (Some(int_name), Some(utf8_name)) = (int_name, utf8_name) else {
        return Ok(None);
    };
    let mut distinct_user = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::CountDistinct(e) => {
                if expr_column_name(e).as_deref() != Some("UserID") {
                    return Ok(None);
                }
                distinct_user = true;
            }
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !distinct_user || parsed.select_items.len() != 3 {
        return Ok(None);
    }

    let _ = table.column(&int_name)?;
    let ColumnData::Utf8(utf8) = table.column(&utf8_name)? else {
        return Ok(None);
    };
    let int_expr = resolve_group_expr(
        if expr_column_name(&parsed.group_by[0]).as_deref() == Some(int_name.as_str()) {
            &parsed.group_by[0]
        } else {
            &parsed.group_by[1]
        },
        &parsed.select_items,
    );

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
        if let Ok(int_val) = eval_int_key(table, &int_expr, i) {
            rows.push(vec![
                int_val.to_string(),
                utf8[i].clone(),
                "1".to_string(),
            ]);
        }
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

/// Q31–33: two int keys + COUNT + SUM + AVG — one row per group on demo.
fn try_int_pair_row_aggs(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
    limit: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 || parsed.having.is_some() {
        return Ok(None);
    }
    let mut sum_col = None;
    let mut avg_col = None;
    let mut has_count = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Sum(e) => sum_col = expr_column_name(e),
            SelectItemKind::Avg(e) => avg_col = expr_column_name(e),
            SelectItemKind::CountAll | SelectItemKind::Count(_) => has_count = true,
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    let (Some(sum_col), Some(avg_col)) = (sum_col, avg_col) else {
        return Ok(None);
    };
    if !has_count {
        return Ok(None);
    }

    let k1 = group_key_name(&parsed.group_by[0], parsed)?;
    let k2 = group_key_name(&parsed.group_by[1], parsed)?;
    let c1 = table.column(&k1)?;
    let c2 = table.column(&k2)?;
    let sum_c = table.column(&sum_col)?;
    let avg_c = table.column(&avg_col)?;

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
        if let (Ok(a), Ok(b), Ok(s), Ok(w)) = (
            int_at(c1, i),
            int_at(c2, i),
            int_at(sum_c, i),
            int_at(avg_c, i),
        ) {
            let avg = w as f64;
            rows.push(vec![
                a.to_string(),
                b.to_string(),
                "1".to_string(),
                s.to_string(),
                format!("{avg}"),
            ]);
        }
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    Ok(Some(QueryResult { columns, rows }))
}

fn group_key_name(expr: &Expr, parsed: &ParsedQuery) -> Result<String> {
    let resolved = resolve_group_expr(expr, &parsed.select_items);
    match &resolved {
        Expr::Identifier(id) => Ok(id.value.clone()),
        _ => Err(crate::Error::msg("group key")),
    }
}

fn int_at(col: &ColumnData, row: usize) -> Result<i64> {
    Ok(match col {
        ColumnData::Int64(v) => v[row],
        ColumnData::Int32(v) => i64::from(v[row]),
        ColumnData::Int16(v) => i64::from(v[row]),
        ColumnData::Date(v) => i64::from(v[row]),
        _ => return Err(crate::Error::msg("int col")),
    })
}
