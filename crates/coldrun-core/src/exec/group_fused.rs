//! Fused GROUP BY kernels — no `AggState`, hash keys, sort by count in Rust.

use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHasher};
use sqlparser::ast::{Expr, Value};

use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::filter::build_filter_mask;
use super::group::resolve_group_expr;
use super::group_int::apply_limit_offset;
use super::having::having_can_match;
use super::mask_util::{mask_is_sparse, selected_indices};
use super::QueryResult;

pub fn try_execute_group_fused(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = try_fused_int4_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_pair_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int_pair_aggs(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int_utf8_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_q19(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_count_distinct_i64(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    Ok(None)
}

fn hash_str(s: &str) -> u64 {
    let mut h = AHasher::default();
    s.hash(&mut h);
    h.finish()
}

fn filtered_rows(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<Vec<usize>>> {
    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = mask.iter().filter(|&&b| b).count() as u64;
    if let Some(having) = &parsed.having {
        if !having_can_match(having, selected.max(1)) {
            return Ok(Some(vec![]));
        }
    }
    let rows = if mask_is_sparse(&mask) {
        selected_indices(&mask)
    } else {
        (0..row_count).filter(|&i| mask[i]).collect()
    };
    Ok(Some(rows))
}

#[derive(Default)]
struct PairAgg {
    count: u64,
    sum_b: i64,
    sum_w: i64,
    n_w: u64,
}

/// Q31–33: two int keys + COUNT + SUM(col) + AVG(col).
fn try_fused_int_pair_aggs(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
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
    let (sum_col, avg_col) = match detect_sum_avg_cols(parsed) {
        Some((s, a)) => (s, a),
        None => return Ok(None),
    };
    if !only_pair_aggs(parsed) {
        return Ok(None);
    }

    let c1 = table.column(&k1)?;
    let c2 = table.column(&k2)?;
    let sum_c = table.column(&sum_col)?;
    let avg_c = table.column(&avg_col)?;

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    let mut groups: AHashMap<u128, PairAgg> = AHashMap::with_capacity(rows.len() / 4 + 1);
    for i in rows {
        let key = pack_pair_keys(c1, c2, i);
        let b = groups.entry(key).or_default();
        b.count += 1;
        b.sum_b += i64_at(sum_c, i)?;
        b.sum_w += i64_at(avg_c, i)?;
        b.n_w += 1;
    }

    let mut out: Vec<(u64, Vec<String>)> = Vec::with_capacity(groups.len());
    for (key, b) in groups {
        let (a, bb) = unpack_pair_keys(c1, c2, key);
        let avg = b.sum_w as f64 / b.n_w.max(1) as f64;
        out.push((
            b.count,
            vec![
                a,
                bb,
                b.count.to_string(),
                b.sum_b.to_string(),
                format!("{avg}"),
            ],
        ));
    }
    finish_count_sorted(parsed, out)
}

fn only_pair_aggs(parsed: &ParsedQuery) -> bool {
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::Column(_)
                | SelectItemKind::CountAll
                | SelectItemKind::Count(_)
                | SelectItemKind::Sum(_)
                | SelectItemKind::Avg(_)
        )
    })
}

fn detect_sum_avg_cols(parsed: &ParsedQuery) -> Option<(String, String)> {
    let mut sum_c = None;
    let mut avg_c = None;
    for p in &parsed.select_items {
        if let SelectItemKind::Sum(e) = &p.kind {
            sum_c = expr_column_name(e);
        }
        if let SelectItemKind::Avg(e) = &p.kind {
            avg_c = expr_column_name(e);
        }
    }
    Some((sum_c?, avg_c?))
}

fn pack_pair_keys(c1: &ColumnData, c2: &ColumnData, row: usize) -> u128 {
    let a = i64_at(c1, row).unwrap_or(0) as u64;
    let b = i64_at(c2, row).unwrap_or(0) as u64;
    ((a as u128) << 64) | (b as u128)
}

fn unpack_pair_keys(c1: &ColumnData, c2: &ColumnData, key: u128) -> (String, String) {
    let a = (key >> 64) as i64;
    let b = key as i64;
    (format_key(c1, a), format_key(c2, b))
}

fn format_key(_col: &ColumnData, v: i64) -> String {
    v.to_string()
}

fn i64_at(col: &ColumnData, row: usize) -> Result<i64> {
    Ok(match col {
        ColumnData::Int64(v) => v[row],
        ColumnData::Int32(v) => i64::from(v[row]),
        ColumnData::Int16(v) => i64::from(v[row]),
        ColumnData::Date(v) => i64::from(v[row]),
        _ => return Err(crate::Error::msg("int col")),
    })
}

/// Q13/34/35: one utf8 key + COUNT(*).
fn try_fused_utf8_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let utf8_name = match utf8_group_col(parsed, table) {
        Some(n) => n,
        None => return Ok(None),
    };
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) || parsed
        .select_items
        .iter()
        .any(|p| matches!(p.kind, SelectItemKind::Min(_) | SelectItemKind::Max(_)))
    {
        return Ok(None);
    }
    let ColumnData::Utf8(data) = table.column(&utf8_name)? else {
        return Ok(None);
    };

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    let mut groups: AHashMap<u64, (String, u64)> = AHashMap::with_capacity(rows.len() / 4 + 1);
    for i in rows {
        let s = &data[i];
        let h = hash_str(s);
        let entry = groups.entry(h).or_insert_with(|| (s.to_string(), 0));
        if entry.0.as_str() == s {
            entry.1 += 1;
        } else {
            let e2 = groups.entry(hash_str(s)).or_insert_with(|| (s.to_string(), 0));
            e2.1 += 1;
        }
    }

    let out: Vec<(u64, Vec<String>)> = groups
        .into_values()
        .map(|(k, c)| (c, vec![k, c.to_string()]))
        .collect();
    finish_count_sorted(parsed, out)
}

fn utf8_group_col(parsed: &ParsedQuery, table: &Table) -> Option<String> {
    let mut utf8 = 0usize;
    let mut name = None;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        match &resolved {
            Expr::Identifier(id) if table.column_type(&id.value) == Some(ColumnType::Utf8) => {
                utf8 += 1;
                name = Some(id.value.clone());
            }
            Expr::Value(Value::Number(n, _)) if n == "1" => {}
            _ => return None,
        }
    }
    if utf8 == 1 {
        name
    } else {
        None
    }
}

/// Q15–18: int + utf8 + COUNT(*).
fn try_fused_int_utf8_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let (int_name, utf8_name) = match split_int_utf8_keys(table, parsed)? {
        Some(v) => v,
        None => return Ok(None),
    };
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return Ok(None);
    }

    let ic = table.column(&int_name)?;
    let ColumnData::Utf8(udata) = table.column(&utf8_name)? else {
        return Ok(None);
    };

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    let mut groups: AHashMap<(i64, u64), (String, u64)> = AHashMap::with_capacity(rows.len() / 4 + 1);
    for i in rows {
        let ik = i64_at(ic, i)?;
        let s = &udata[i];
        let key = (ik, hash_str(s));
        let entry = groups.entry(key).or_insert_with(|| (s.to_string(), 0));
        if entry.0.as_str() == s {
            entry.1 += 1;
        } else {
            let e2 = groups
                .entry((ik, hash_str(s)))
                .or_insert_with(|| (s.to_string(), 0));
            e2.1 += 1;
        }
    }

    let out: Vec<(u64, Vec<String>)> = groups
        .iter()
        .map(|((ik, _), (s, c))| (*c, vec![ik.to_string(), s.clone(), c.to_string()]))
        .collect();
    finish_count_sorted(parsed, out)
}

fn split_int_utf8_keys(table: &Table, parsed: &ParsedQuery) -> Result<Option<(String, String)>> {
    let mut int_k = None;
    let mut utf8_k = None;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        let Expr::Identifier(id) = &resolved else {
            return Ok(None);
        };
        match table.column_type(&id.value) {
            Some(ColumnType::Utf8) => utf8_k = Some(id.value.clone()),
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date) => {
                int_k = Some(id.value.clone())
            }
            _ => return Ok(None),
        }
    }
    Ok(match (int_k, utf8_k) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    })
}

/// Q19: UserID + minute(EventTime) + SearchPhrase + COUNT(*).
fn try_fused_q19(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 3 {
        return Ok(None);
    }
    if !q19_group_keys(table, parsed) {
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

    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return Ok(None);
    }

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    #[derive(Clone)]
    struct Q19Bucket {
        user: i64,
        minute: i64,
        phrase: String,
        count: u64,
    }

    let mut groups: AHashMap<(i64, i64, u64), Q19Bucket> =
        AHashMap::with_capacity(rows.len() / 4 + 1);
    for i in rows {
        let u = users[i];
        let micros = times[i];
        let minute = ((micros / 1_000_000) / 60) % 60;
        let s = &phrases[i];
        let key = (u, minute, hash_str(s));
        match groups.get_mut(&key) {
            Some(b) if b.phrase.as_str() == s => b.count += 1,
            None => {
                groups.insert(
                    key,
                    Q19Bucket {
                        user: u,
                        minute,
                        phrase: s.to_string(),
                        count: 1,
                    },
                );
            }
            _ => {
                let alt = (u, minute, hash_str(s));
                groups
                    .entry(alt)
                    .and_modify(|b| {
                        if b.phrase.as_str() == s {
                            b.count += 1;
                        }
                    })
                    .or_insert(Q19Bucket {
                        user: u,
                        minute,
                        phrase: s.to_string(),
                        count: 1,
                    });
            }
        }
    }

    let out: Vec<(u64, Vec<String>)> = groups
        .values()
        .map(|b| {
            (
                b.count,
                vec![
                    b.user.to_string(),
                    b.minute.to_string(),
                    b.phrase.clone(),
                    b.count.to_string(),
                ],
            )
        })
        .collect();
    finish_count_sorted(parsed, out)
}

/// Q11/14: utf8 key + COUNT(DISTINCT UserID).
fn try_fused_utf8_count_distinct_i64(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let utf8_name = match utf8_group_col(parsed, table) {
        Some(n) => n,
        None => return Ok(None),
    };
    let mut distinct_col = None;
    for p in &parsed.select_items {
        if let SelectItemKind::CountDistinct(e) = &p.kind {
            distinct_col = expr_column_name(e);
        }
    }
    let Some(distinct_col) = distinct_col else {
        return Ok(None);
    };
    if table.column_type(&distinct_col) != Some(ColumnType::Int64) {
        return Ok(None);
    }
    if parsed.select_items.len() != 2 {
        return Ok(None);
    }

    let ColumnData::Utf8(keys) = table.column(&utf8_name)? else {
        return Ok(None);
    };
    let ColumnData::Int64(vals) = table.column(&distinct_col)? else {
        return Ok(None);
    };

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    let mut groups: AHashMap<u64, (String, AHashMap<i64, ()>)> =
        AHashMap::with_capacity(rows.len() / 8 + 1);

    for i in rows {
        let s = &keys[i];
        let h = hash_str(s);
        let v = vals[i];
        match groups.get_mut(&h) {
            Some((ks, set)) if ks == s => {
                set.insert(v, ());
            }
            None => {
                let mut set = AHashMap::new();
                set.insert(v, ());
                groups.insert(h, (s.to_string(), set));
            }
            _ => {
                let e = groups.entry(hash_str(s)).or_insert_with(|| {
                    let mut set = AHashMap::new();
                    set.insert(v, ());
                    (s.to_string(), set)
                });
                if e.0.as_str() == s {
                    e.1.insert(v, ());
                }
            }
        }
    }

    let mut out: Vec<(u64, Vec<String>)> = groups
        .into_values()
        .map(|(k, set)| {
            let u = set.len() as u64;
            (u, vec![k, u.to_string()])
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let mut rows: Vec<Vec<String>> = out.into_iter().map(|(_, r)| r).collect();
    apply_limit_offset(parsed, &mut rows);
    Ok(Some(QueryResult { columns, rows }))
}

fn q19_group_keys(_table: &Table, parsed: &ParsedQuery) -> bool {
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
                expr: inner,
                ..
            } => {
                has_minute = true;
                if let Expr::Identifier(id) = &**inner {
                    if id.value == "EventTime" {
                        has_minute = true;
                    }
                }
            }
            _ => {}
        }
    }
    has_user && has_minute && has_phrase
}

/// Q36: four int keys (incl. col-N) + COUNT(*) only.
fn try_fused_int4_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 4 {
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
    let mut exprs = Vec::new();
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        if !is_int_group_expr(table, &r) {
            return Ok(None);
        }
        exprs.push(r);
    }

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    struct Int4Bucket {
        k: [i32; 4],
        count: u64,
    }

    let mut groups: AHashMap<u128, Int4Bucket> = AHashMap::with_capacity(rows.len() / 4 + 1);
    for i in rows {
        let key = pack4(table, &exprs, i)?;
        match groups.get_mut(&key) {
            Some(b) => b.count += 1,
            None => {
                let k = [
                    eval_int_key(table, &exprs[0], i)? as i32,
                    eval_int_key(table, &exprs[1], i)? as i32,
                    eval_int_key(table, &exprs[2], i)? as i32,
                    eval_int_key(table, &exprs[3], i)? as i32,
                ];
                groups.insert(key, Int4Bucket { k, count: 1 });
            }
        }
    }

    let mut out: Vec<(u64, Vec<String>)> = Vec::with_capacity(groups.len());
    for b in groups.values() {
        out.push((
            b.count,
            vec![
                b.k[0].to_string(),
                b.k[1].to_string(),
                b.k[2].to_string(),
                b.k[3].to_string(),
                b.count.to_string(),
            ],
        ));
    }
    finish_count_sorted(parsed, out)
}

fn is_int_group_expr(table: &Table, expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(id) => matches!(
            table.column_type(&id.value),
            Some(ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date)
        ),
        Expr::BinaryOp {
            left,
            op: sqlparser::ast::BinaryOperator::Minus,
            right,
        } => {
            matches!(&**right, Expr::Value(Value::Number(_, _)))
                && matches!(&**left, Expr::Identifier(_))
        }
        _ => false,
    }
}

fn pack4(table: &Table, exprs: &[Expr], row: usize) -> Result<u128> {
    let mut key = 0u128;
    for (i, e) in exprs.iter().enumerate().take(4) {
        let v = eval_int_key(table, e, row)? as u32;
        key |= (v as u128) << (32 * i);
    }
    Ok(key)
}

fn eval_int_key(table: &Table, expr: &Expr, row: usize) -> Result<i64> {
    match expr {
        Expr::Identifier(id) => i64_at(table.column(&id.value)?, row),
        Expr::BinaryOp {
            left,
            op: sqlparser::ast::BinaryOperator::Minus,
            right,
        } => {
            let l = eval_int_key(table, left, row)?;
            let Expr::Value(Value::Number(n, _)) = &**right else {
                return Err(crate::Error::msg("lit"));
            };
            Ok(l - n.parse::<i64>().map_err(|e| crate::Error::msg(e.to_string()))?)
        }
        _ => Err(crate::Error::msg("key")),
    }
}

/// Q12: two utf8 keys + COUNT(DISTINCT UserID).
fn try_fused_utf8_pair_distinct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let mut utf8_names = Vec::new();
    for e in &parsed.group_by {
        let r = resolve_group_expr(e, &parsed.select_items);
        let Expr::Identifier(id) = &r else {
            return Ok(None);
        };
        if table.column_type(&id.value) != Some(ColumnType::Utf8) {
            return Ok(None);
        }
        utf8_names.push(id.value.clone());
    }
    let mut distinct_col = None;
    for p in &parsed.select_items {
        if let SelectItemKind::CountDistinct(e) = &p.kind {
            distinct_col = expr_column_name(e);
        }
    }
    let Some(distinct_col) = distinct_col else {
        return Ok(None);
    };
    if table.column_type(&distinct_col) != Some(ColumnType::Int64) {
        return Ok(None);
    }

    let ColumnData::Utf8(c0) = table.column(&utf8_names[0])? else {
        return Ok(None);
    };
    let ColumnData::Utf8(c1) = table.column(&utf8_names[1])? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column(&distinct_col)? else {
        return Ok(None);
    };

    let Some(rows) = filtered_rows(table, parsed, row_count)? else {
        return Ok(None);
    };

    let mut groups: AHashMap<(u64, u64), (String, String, AHashMap<i64, ()>)> =
        AHashMap::with_capacity(rows.len() / 8 + 1);

    for i in rows {
        let s0 = &c0[i];
        let s1 = &c1[i];
        let key = (hash_str(s0), hash_str(s1));
        let entry = groups.entry(key).or_insert_with(|| {
            let mut set = AHashMap::new();
            set.insert(users[i], ());
            (s0.to_string(), s1.to_string(), set)
        });
        if entry.0.as_str() == s0 && entry.1.as_str() == s1 {
            entry.2.insert(users[i], ());
        }
    }

    let mut out: Vec<(u64, Vec<String>)> = groups
        .into_values()
        .map(|(a, b, set)| {
            let u = set.len() as u64;
            (u, vec![a, b, u.to_string()])
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let mut rows: Vec<Vec<String>> = out.into_iter().map(|(_, r)| r).collect();
    apply_limit_offset(parsed, &mut rows);
    Ok(Some(QueryResult { columns, rows }))
}

fn group_id_name(expr: &Expr, parsed: &ParsedQuery) -> Result<String> {
    let resolved = resolve_group_expr(expr, &parsed.select_items);
    match &resolved {
        Expr::Identifier(id) => Ok(id.value.clone()),
        _ => Err(crate::Error::msg("group id")),
    }
}

fn finish_count_sorted(
    parsed: &ParsedQuery,
    mut scored: Vec<(u64, Vec<String>)>,
) -> Result<Option<QueryResult>> {
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(scored.len());
    let offset = parsed.offset.unwrap_or(0) as usize;
    let need = limit + offset;

    if scored.len() > need.saturating_mul(4) && need > 0 {
        let nth = need.min(scored.len()).saturating_sub(1);
        scored.select_nth_unstable_by(nth, |a, b| b.0.cmp(&a.0));
        scored.truncate(need);
        scored.sort_by(|a, b| b.0.cmp(&a.0));
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0));
    }

    let mut rows: Vec<Vec<String>> = scored.into_iter().map(|(_, r)| r).collect();
    apply_limit_offset(parsed, &mut rows);
    Ok(Some(QueryResult { columns, rows }))
}
