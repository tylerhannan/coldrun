use ahash::AHashSet;

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::expr::format_date_days;
use crate::sql::{expr_column_name, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::mask_util::for_each_selected;
use super::simd_count::{count_i16_ne_zero, count_i32_ne_zero, count_i64_ne_zero};
use super::QueryResult;
use super::{build_filter_mask, projection_label};

/// Fast path for single-row global aggregates (no GROUP BY).
pub fn try_execute_global(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !parsed.group_by.is_empty() {
        return Ok(None);
    }

    if parsed.select_items.len() > 1 {
        if let Some(r) = try_execute_global_minmax_pair(table, parsed, row_count)? {
            return Ok(Some(r));
        }
        if let Some(r) = try_execute_global_multi_distinct(table, parsed, row_count)? {
            return Ok(Some(r));
        }
        return try_execute_global_multi(table, parsed, row_count);
    }

    // Q6: global COUNT(DISTINCT SearchPhrase) — no WHERE.
    if let Some(r) = try_count_distinct_searchphrase(table, parsed, row_count)? {
        return Ok(Some(r));
    }

    let proj = &parsed.select_items[0];
    let col_name = projection_label(proj);

    // Q1: bare COUNT(*) — metadata row count, no column scan.
    if parsed.where_expr.is_none() {
        if matches!(proj.kind, SelectItemKind::CountAll | SelectItemKind::Count(_)) {
            return Ok(Some(QueryResult {
                columns: vec![col_name],
                rows: vec![vec![row_count.to_string()]],
            }));
        }
    }

    // Q2: `COUNT(*) WHERE col <> 0` on int columns — count non-zeros directly.
    if let Some(n) = try_count_int_nonzero(table, parsed.where_expr.as_ref(), row_count)? {
        if matches!(proj.kind, SelectItemKind::CountAll | SelectItemKind::Count(_)) {
            return Ok(Some(QueryResult {
                columns: vec![col_name],
                rows: vec![vec![n.to_string()]],
            }));
        }
    }

    // Q21: `COUNT(*) WHERE URL LIKE '%…%'` — scan URL column only, no mask alloc.
    if let Some(n) = try_count_utf8_like(table, parsed.where_expr.as_ref(), row_count)? {
        if matches!(proj.kind, SelectItemKind::CountAll | SelectItemKind::Count(_)) {
            return Ok(Some(QueryResult {
                columns: vec![col_name],
                rows: vec![vec![n.to_string()]],
            }));
        }
    }

    let mask = build_filter_mask(&table, parsed.where_expr.as_ref(), row_count)?;
    let selected = count_selected(&mask);

    match &proj.kind {
        SelectItemKind::CountAll | SelectItemKind::Count(_) => {
            return Ok(Some(QueryResult {
                columns: vec![col_name],
                rows: vec![vec![selected.to_string()]],
            }));
        }
        SelectItemKind::Sum(expr) => {
            if let Some(name) = expr_column_name(expr) {
                if let Ok(sum) = sum_column_masked(table, &name, &mask) {
                    return Ok(Some(QueryResult {
                        columns: vec![col_name],
                        rows: vec![vec![sum.to_string()]],
                    }));
                }
            }
        }
        SelectItemKind::Avg(expr) => {
            if let Some(name) = expr_column_name(expr) {
                if let Ok((sum, n)) = sum_column_masked_with_count(table, &name, &mask) {
                    if n > 0 {
                        let avg = sum / n as f64;
                        return Ok(Some(QueryResult {
                            columns: vec![col_name],
                            rows: vec![vec![format!("{avg}")]],
                        }));
                    }
                }
            }
        }
        SelectItemKind::CountDistinct(expr) => {
            if let Some(n) = count_distinct_masked(table, expr, &mask, row_count) {
                return Ok(Some(QueryResult {
                    columns: vec![col_name],
                    rows: vec![vec![n.to_string()]],
                }));
            }
        }
        SelectItemKind::Min(expr) | SelectItemKind::Max(expr) => {
            if let Some(v) =
                minmax_column_masked(table, expr, &mask, matches!(proj.kind, SelectItemKind::Max(_)))
            {
                return Ok(Some(QueryResult {
                    columns: vec![col_name],
                    rows: vec![vec![v]],
                }));
            }
        }
        _ => {}
    }
    Ok(None)
}

/// Q7: `SELECT MIN(EventDate), MAX(EventDate)` in one pass.
fn try_execute_global_minmax_pair(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.select_items.len() != 2 {
        return Ok(None);
    }
    let mut min_expr = None;
    let mut max_expr = None;
    for proj in &parsed.select_items {
        match &proj.kind {
            SelectItemKind::Min(e) => min_expr = Some((proj, e)),
            SelectItemKind::Max(e) => max_expr = Some((proj, e)),
            _ => return Ok(None),
        }
    }
    let (min_proj, min_e, max_proj, max_e) = match (min_expr, max_expr) {
        (Some((mp, me)), Some((xp, xe))) => (mp, me, xp, xe),
        _ => return Ok(None),
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let Some(min_v) = minmax_column_masked(table, min_e, &mask, false) else {
        return Ok(None);
    };
    let Some(max_v) = minmax_column_masked(table, max_e, &mask, true) else {
        return Ok(None);
    };

    Ok(Some(QueryResult {
        columns: vec![
            projection_label(min_proj),
            projection_label(max_proj),
        ],
        rows: vec![vec![min_v, max_v]],
    }))
}

/// One mask pass for Q3-style `SELECT SUM(..), COUNT(*), AVG(..)` on int columns.
fn try_execute_global_multi(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let mut plans = Vec::with_capacity(parsed.select_items.len());
    for proj in &parsed.select_items {
        plans.push(match classify_simple(&proj.kind)? {
            Some(p) => p,
            None => return Ok(None),
        });
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = count_selected(&mask);

    let mut columns = Vec::with_capacity(parsed.select_items.len());
    let mut values = Vec::with_capacity(parsed.select_items.len());

    for (proj, plan) in parsed.select_items.iter().zip(plans.iter()) {
        columns.push(projection_label(proj));
        values.push(match plan {
            SimpleAgg::CountAll => selected.to_string(),
            SimpleAgg::Sum(name) => sum_column_masked(table, name, &mask)?.to_string(),
            SimpleAgg::Avg(name) => {
                let (sum, n) = sum_column_masked_with_count(table, name, &mask)?;
                let avg = sum / n.max(1) as f64;
                format!("{avg}")
            }
            SimpleAgg::CountDistinct(name) => count_distinct_col_masked(table, name, &mask, row_count)
                .ok_or_else(|| crate::Error::msg("count distinct"))?
                .to_string(),
        });
    }

    Ok(Some(QueryResult {
        columns,
        rows: vec![values],
    }))
}

enum SimpleAgg {
    CountAll,
    Sum(String),
    Avg(String),
    CountDistinct(String),
}

/// Q5+Q6 style: two global `COUNT(DISTINCT col)` in one query (one mask pass).
fn try_execute_global_multi_distinct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.select_items.len() != 2 {
        return Ok(None);
    }
    let mut names = Vec::new();
    for proj in &parsed.select_items {
        match &proj.kind {
            SelectItemKind::CountDistinct(e) => {
                names.push(expr_column_name(e).ok_or_else(|| crate::Error::msg("col"))?);
            }
            _ => return Ok(None),
        }
    }

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let mut columns = Vec::new();
    let mut values = Vec::new();
    for (proj, name) in parsed.select_items.iter().zip(names.iter()) {
        columns.push(projection_label(proj));
        values.push(
            count_distinct_col_masked(table, name, &mask, row_count)
                .ok_or_else(|| crate::Error::msg("count distinct"))?
                .to_string(),
        );
    }
    Ok(Some(QueryResult {
        columns,
        rows: vec![values],
    }))
}

fn classify_simple(kind: &SelectItemKind) -> Result<Option<SimpleAgg>> {
    Ok(match kind {
        SelectItemKind::CountAll | SelectItemKind::Count(_) => Some(SimpleAgg::CountAll),
        SelectItemKind::Sum(e) => expr_column_name(e).map(SimpleAgg::Sum),
        SelectItemKind::Avg(e) => expr_column_name(e).map(SimpleAgg::Avg),
        SelectItemKind::CountDistinct(e) => expr_column_name(e).map(SimpleAgg::CountDistinct),
        _ => None,
    })
}

fn try_count_distinct_searchphrase(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.where_expr.is_some() || parsed.group_by.len() > 0 {
        return Ok(None);
    }
    if parsed.select_items.len() != 1 {
        return Ok(None);
    }
    let proj = &parsed.select_items[0];
    let SelectItemKind::CountDistinct(e) = &proj.kind else {
        return Ok(None);
    };
    if crate::sql::expr_column_name(e).as_deref() != Some("SearchPhrase") {
        return Ok(None);
    }
    let ColumnData::Utf8(v) = table.column("SearchPhrase")? else {
        return Ok(None);
    };
    use std::hash::{Hash, Hasher};
    use ahash::AHasher;

    let mut ids = ahash::AHashSet::with_capacity(4096);
    let mut has_empty = false;
    for s in v.iter().take(row_count) {
        if s.is_empty() {
            has_empty = true;
            continue;
        }
        let mut h = AHasher::default();
        s.hash(&mut h);
        ids.insert(h.finish());
    }
    let n = ids.len() + usize::from(has_empty);
    Ok(Some(QueryResult {
        columns: vec![projection_label(proj)],
        rows: vec![vec![n.to_string()]],
    }))
}

fn try_count_int_nonzero(
    table: &Table,
    where_expr: Option<&Expr>,
    row_count: usize,
) -> Result<Option<u64>> {
    let Some(expr) = where_expr else {
        return Ok(None);
    };
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::NotEq,
        right,
    } = expr
    else {
        return Ok(None);
    };
    let Some(name) = expr_column_name(left) else {
        return Ok(None);
    };
    let Expr::Value(Value::Number(n, _)) = &**right else {
        return Ok(None);
    };
    if n != "0" {
        return Ok(None);
    }
    if name == "AdvEngineID" {
        if let Some(n) = table.zones().and_then(|z| z.count_adv_nonzero_total()) {
            return Ok(Some(n));
        }
    }
    let col = table.column(&name)?;
    let count = match col {
        ColumnData::Int16(v) => count_i16_ne_zero(&v[..row_count.min(v.len())]),
        ColumnData::Int32(v) => count_i32_ne_zero(&v[..row_count.min(v.len())]),
        ColumnData::Int64(v) => count_i64_ne_zero(&v[..row_count.min(v.len())]),
        _ => return Ok(None),
    };
    Ok(Some(count))
}

fn try_count_utf8_like(
    table: &Table,
    where_expr: Option<&Expr>,
    row_count: usize,
) -> Result<Option<u64>> {
    let Some(expr) = where_expr else {
        return Ok(None);
    };
    let Expr::Like {
        expr: inner,
        pattern,
        negated: false,
        ..
    } = expr
    else {
        return Ok(None);
    };
    let Some(name) = expr_column_name(inner) else {
        return Ok(None);
    };
    let Expr::Value(v) = &**pattern else {
        return Ok(None);
    };
    let pat = match v {
        Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => s.as_str(),
        _ => return Ok(None),
    };
    let col = table.column(&name)?;
    let ColumnData::Utf8(data) = col else {
        return Ok(None);
    };
    let n = data
        .iter()
        .take(row_count)
        .filter(|s| crate::expr::eval_like_match(s, pat))
        .count() as u64;
    Ok(Some(n))
}

fn count_selected(mask: &[bool]) -> u64 {
    mask.iter().map(|&b| u64::from(b)).sum()
}

fn count_distinct_masked(
    table: &Table,
    expr: &Expr,
    mask: &[bool],
    row_count: usize,
) -> Option<u64> {
    let name = expr_column_name(expr)?;
    count_distinct_col_masked(table, &name, mask, row_count)
}

fn count_distinct_col_masked(
    table: &Table,
    name: &str,
    mask: &[bool],
    row_count: usize,
) -> Option<u64> {
    let col = table.column(name).ok()?;
    match col {
        ColumnData::Int64(v) => {
            let mut set = AHashSet::new();
            for_each_selected(mask, row_count, |i| {
                set.insert(v[i]);
            });
            Some(set.len() as u64)
        }
        ColumnData::Int32(v) => {
            let mut set = AHashSet::new();
            for_each_selected(mask, row_count, |i| {
                set.insert(i64::from(v[i]));
            });
            Some(set.len() as u64)
        }
        ColumnData::Int16(v) => {
            let mut set = AHashSet::new();
            for_each_selected(mask, row_count, |i| {
                set.insert(i64::from(v[i]));
            });
            Some(set.len() as u64)
        }
        ColumnData::Utf8(v) => {
            let mut intern = super::utf8_arena::Utf8Intern::with_capacity(4096);
            let mut ids = AHashSet::new();
            for_each_selected(mask, row_count, |i| {
                ids.insert(intern.intern(v.get(i)));
            });
            Some(ids.len() as u64)
        }
        _ => None,
    }
}

fn sum_column_masked(table: &Table, name: &str, mask: &[bool]) -> Result<i128> {
    let (sum, _) = sum_column_masked_with_count(table, name, mask)?;
    Ok(sum.round() as i128)
}

fn sum_column_masked_with_count(table: &Table, name: &str, mask: &[bool]) -> Result<(f64, u64)> {
    let col = table.column(name)?;
    let mut sum = 0.0f64;
    let mut n = 0u64;
    match col {
        ColumnData::Int64(v) => {
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    sum += x as f64;
                    n += 1;
                }
            }
        }
        ColumnData::Int32(v) => {
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    sum += f64::from(x);
                    n += 1;
                }
            }
        }
        ColumnData::Int16(v) => {
            for (i, &x) in v.iter().enumerate() {
                if mask.get(i).copied().unwrap_or(false) {
                    sum += f64::from(x);
                    n += 1;
                }
            }
        }
        _ => return Err(crate::Error::msg("sum on non-int column")),
    }
    Ok((sum, n))
}

fn minmax_column_masked(table: &Table, expr: &Expr, mask: &[bool], is_max: bool) -> Option<String> {
    let name = expr_column_name(expr)?;
    let col = table.column(&name).ok()?;
    match col {
        ColumnData::Date(v) => {
            let mut opt: Option<i32> = None;
            for (i, &x) in v.iter().enumerate() {
                if !mask.get(i).copied().unwrap_or(false) {
                    continue;
                }
                opt = Some(match opt {
                    None => x,
                    Some(cur) if is_max => cur.max(x),
                    Some(cur) => cur.min(x),
                });
            }
            return opt.map(format_date_days);
        }
        ColumnData::Int64(v) => {
            let mut opt: Option<i64> = None;
            for (i, &x) in v.iter().enumerate() {
                if !mask.get(i).copied().unwrap_or(false) {
                    continue;
                }
                opt = Some(match opt {
                    None => x,
                    Some(cur) if is_max => cur.max(x),
                    Some(cur) => cur.min(x),
                });
            }
            return opt.map(|d| d.to_string());
        }
        _ => {}
    }
    None
}
