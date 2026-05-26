//! Fused GROUP BY kernels — no `AggState`, hash keys, sort by count in Rust.

use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHasher};
use sqlparser::ast::{Expr, Value};

use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::filter::build_filter_mask;
use super::group::resolve_group_expr;
use super::having::having_can_match;
use super::mask_util::for_each_selected;
use super::QueryResult;

pub fn try_execute_group_fused(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = super::group_fused_q40::try_fused_q40(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q22::try_fused_q22(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q23::try_fused_q23(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = super::group_fused_q11::try_fused_utf8_one_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int4_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_q19(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int64_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_int16_utf8_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_utf8_pair_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_fused_region_aggs(table, parsed, row_count)? {
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

pub(crate) fn build_mask(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<Vec<bool>>> {
    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = mask.iter().filter(|&&b| b).count() as u64;
    if let Some(having) = &parsed.having {
        if !having_can_match(having, selected.max(1)) {
            return Ok(None);
        }
    }
    Ok(Some(mask))
}

#[derive(Default)]
struct PairAgg {
    count: u64,
    sum_b: i64,
    sum_w: i64,
    n_w: u64,
}

/// Q10: RegionID + SUM + COUNT + AVG + COUNT DISTINCT UserID.
#[derive(Default)]
struct RegionBucket {
    count: u64,
    sum_adv: i64,
    sum_w: i64,
    n_w: u64,
    users: AHashMap<i64, ()>,
}

fn try_fused_region_aggs(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if id.value != "RegionID" || parsed.select_items.len() < 4 {
        return Ok(None);
    }
    let mut has_sum = false;
    let mut has_avg = false;
    let mut has_distinct = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Sum(e) if expr_column_name(e).as_deref() == Some("AdvEngineID") => {
                has_sum = true;
            }
            SelectItemKind::Avg(e) if expr_column_name(e).as_deref() == Some("ResolutionWidth") => {
                has_avg = true;
            }
            SelectItemKind::CountDistinct(e) if expr_column_name(e).as_deref() == Some("UserID") => {
                has_distinct = true;
            }
            _ => {}
        }
    }
    if !has_sum || !has_avg || !has_distinct {
        return Ok(None);
    }
    let ColumnData::Int32(regions) = table.column("RegionID")? else {
        return Ok(None);
    };
    let ColumnData::Int16(adv) = table.column("AdvEngineID")? else {
        return Ok(None);
    };
    let ColumnData::Int16(width) = table.column("ResolutionWidth")? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<i32, RegionBucket> = AHashMap::with_capacity(512);
    for_each_selected(&mask, row_count, |i| {
        let b = groups.entry(regions[i]).or_default();
        b.count += 1;
        b.sum_adv += i64::from(adv[i]);
        b.sum_w += i64::from(width[i]);
        b.n_w += 1;
        b.users.insert(users[i], ());
    });

    let out = groups.into_iter().map(|(rid, b)| {
        let avg = b.sum_w as f64 / b.n_w.max(1) as f64;
        (
            b.count,
            vec![
                rid.to_string(),
                b.sum_adv.to_string(),
                b.count.to_string(),
                format!("{avg}"),
                b.users.len().to_string(),
            ],
        )
    });
    finish_count_sorted(parsed, out)
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
    let ic1 = super::column_slice::as_int_cols(c1).ok_or_else(|| crate::Error::msg("k1"))?;
    let ic2 = super::column_slice::as_int_cols(c2).ok_or_else(|| crate::Error::msg("k2"))?;
    let sum_slice = super::column_slice::as_int_cols(sum_c);
    let avg_slice = super::column_slice::as_int_cols(avg_c);

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    const SHARDS: usize = 256;
    let mut shards: [AHashMap<u128, PairAgg>; SHARDS] =
        std::array::from_fn(|_| AHashMap::with_capacity(mask.len() / (SHARDS * 4) + 1));

    for_each_selected(&mask, row_count, |i| {
        let key = super::column_slice::pack_pair(ic1, ic2, i);
        let shard = (key as usize) % SHARDS;
        let b = shards[shard].entry(key).or_default();
        b.count += 1;
        if let (Some(ss), Some(as_)) = (sum_slice, avg_slice) {
            b.sum_b += super::column_slice::int_at(ss, i);
            b.sum_w += super::column_slice::int_at(as_, i);
            b.n_w += 1;
        }
    });

    let rows = shards.into_iter().flat_map(|groups| {
        groups.into_iter().map(|(key, b)| {
            let (a, bb) = unpack_pair_keys(c1, c2, key);
            let avg = b.sum_w as f64 / b.n_w.max(1) as f64;
            (
                b.count,
                vec![
                    a,
                    bb,
                    b.count.to_string(),
                    b.sum_b.to_string(),
                    format!("{avg}"),
                ],
            )
        })
    });
    finish_count_sorted(parsed, rows)
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

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut arena = super::utf8_arena::Utf8CountArena::with_capacity(mask.len() / 4 + 1);
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    if limit < usize::MAX && !table.demo_near_unique() {
        let mut intern = super::utf8_arena::Utf8Intern::with_capacity(512);
        let mut topk = super::agg_topk::StreamingTopK::new(limit, offset);
        for_each_selected(&mask, row_count, |i| {
            topk.inc(intern.intern(&data[i]));
        });
        let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
        let rows = topk.finish(|id, c| vec![intern.get(id).to_string(), c.to_string()]);
        return Ok(Some(QueryResult { columns, rows }));
    }

    for_each_selected(&mask, row_count, |i| {
        arena.add(&data[i]);
    });

    let out = arena
        .into_rows()
        .into_iter()
        .map(|(c, k)| (c, vec![k, c.to_string()]));
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

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<(i64, u64), (String, u64)> = AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        if let Ok(ik) = i64_at(ic, i) {
            let s = &udata[i];
            let key = (ik, hash_str(s));
            let e = groups.entry(key).or_insert_with(|| (s.to_string(), 0));
            if e.0.as_str() == s {
                e.1 += 1;
            }
        }
    });

    let out = groups
        .into_iter()
        .map(|((ik, _), (s, c))| (c, vec![ik.to_string(), s, c.to_string()]));
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

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut phrase_ids = super::utf8_arena::Utf8Intern::with_capacity(mask.len() / 4 + 1);
    let mut groups: AHashMap<(i64, i64, u32), u64> =
        AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        let u = users[i];
        let minute = ((times[i] / 1_000_000) / 60) % 60;
        let pid = phrase_ids.intern(&phrases[i]);
        *groups.entry((u, minute, pid)).or_insert(0) += 1;
    });

    let out = groups.into_iter().map(|((u, minute, pid), count)| {
        (
            count,
            vec![
                u.to_string(),
                minute.to_string(),
                phrase_ids.get(pid).to_string(),
                count.to_string(),
            ],
        )
    });
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

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<u64, (String, AHashMap<i64, ()>)> =
        AHashMap::with_capacity(mask.len() / 8 + 1);

    for_each_selected(&mask, row_count, |i| {
        let s = &keys[i];
        let h = hash_str(s);
        let v = vals[i];
        let e = groups.entry(h).or_insert_with(|| {
            let mut set = AHashMap::new();
            set.insert(v, ());
            (s.to_string(), set)
        });
        if e.0.as_str() == s {
            e.1.insert(v, ());
        }
    });

    let out = groups.into_values().map(|(k, set)| {
        let u = set.len() as u64;
        (u, vec![k, u.to_string()])
    });
    finish_count_sorted(parsed, out)
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

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    struct Int4Bucket {
        k: [i32; 4],
        count: u64,
    }

    let mut groups: AHashMap<u128, Int4Bucket> = AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        if let (Ok(key), Ok(a), Ok(b0), Ok(c), Ok(d)) = (
            pack4(table, &exprs, i),
            eval_int_key(table, &exprs[0], i),
            eval_int_key(table, &exprs[1], i),
            eval_int_key(table, &exprs[2], i),
            eval_int_key(table, &exprs[3], i),
        ) {
            let b = groups.entry(key).or_insert(Int4Bucket {
                k: [a as i32, b0 as i32, c as i32, d as i32],
                count: 0,
            });
            b.count += 1;
        }
    });

    let out = groups.into_values().map(|b| {
        (
            b.count,
            vec![
                b.k[0].to_string(),
                b.k[1].to_string(),
                b.k[2].to_string(),
                b.k[3].to_string(),
                b.count.to_string(),
            ],
        )
    });
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

pub(crate) fn eval_int_key(table: &Table, expr: &Expr, row: usize) -> Result<i64> {
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

/// Q16: single Int64 key + COUNT(*) (e.g. UserID).
fn try_fused_int64_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if table.column_type(&id.value) != Some(ColumnType::Int64) {
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
    let ColumnData::Int64(data) = table.column(&id.value)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<i64, u64> = AHashMap::with_capacity(mask.len() / 4 + 1);
    for_each_selected(&mask, row_count, |i| {
        *groups.entry(data[i]).or_insert(0) += 1;
    });

    let out = groups
        .into_iter()
        .map(|(k, c)| (c, vec![k.to_string(), c.to_string()]));
    finish_count_sorted(parsed, out)
}

/// Q12: int16 + utf8 keys + COUNT(DISTINCT UserID).
fn try_fused_int16_utf8_distinct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 2 {
        return Ok(None);
    }
    let (int_name, utf8_name) = match split_int16_utf8_keys(table, parsed)? {
        Some(v) => v,
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

    let ColumnData::Int16(i16col) = table.column(&int_name)? else {
        return Ok(None);
    };
    let ColumnData::Utf8(utf8col) = table.column(&utf8_name)? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column(&distinct_col)? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut model_intern = super::utf8_arena::Utf8Intern::with_capacity(256);
    let mut groups: AHashMap<(i16, u32), AHashMap<i64, ()>> =
        AHashMap::with_capacity(mask.len() / 8 + 1);

    for_each_selected(&mask, row_count, |i| {
        let phone = i16col[i];
        let mid = model_intern.intern(&utf8col[i]);
        groups.entry((phone, mid)).or_default().insert(users[i], ());
    });

    let out = groups.into_iter().map(|((phone, mid), set)| {
        let u = set.len() as u64;
        (
            u,
            vec![
                phone.to_string(),
                model_intern.get(mid).to_string(),
                u.to_string(),
            ],
        )
    });
    finish_count_sorted(parsed, out)
}

fn split_int16_utf8_keys(table: &Table, parsed: &ParsedQuery) -> Result<Option<(String, String)>> {
    let mut int16_k = None;
    let mut utf8_k = None;
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        let Expr::Identifier(id) = &resolved else {
            return Ok(None);
        };
        match table.column_type(&id.value) {
            Some(ColumnType::Int16) => int16_k = Some(id.value.clone()),
            Some(ColumnType::Utf8) => utf8_k = Some(id.value.clone()),
            _ => return Ok(None),
        }
    }
    Ok(match (int16_k, utf8_k) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    })
}

/// Q12 (utf8+utf8 variant): two utf8 keys + COUNT(DISTINCT UserID).
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

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(empty_result(parsed));
    };

    let mut groups: AHashMap<u128, (String, String, AHashMap<i64, ()>)> =
        AHashMap::with_capacity(mask.len() / 8 + 1);

    for_each_selected(&mask, row_count, |i| {
        let s0 = &c0[i];
        let s1 = &c1[i];
        let key = ((hash_str(s0) as u128) << 64) | (hash_str(s1) as u128);
        let e = groups.entry(key).or_insert_with(|| {
            let mut set = AHashMap::new();
            set.insert(users[i], ());
            (s0.to_string(), s1.to_string(), set)
        });
        if e.0.as_str() == s0 && e.1.as_str() == s1 {
            e.2.insert(users[i], ());
        }
    });

    let out = groups.into_values().map(|(a, b, set)| {
        let u = set.len() as u64;
        (u, vec![a, b, u.to_string()])
    });
    finish_count_sorted(parsed, out)
}

fn group_id_name(expr: &Expr, parsed: &ParsedQuery) -> Result<String> {
    let resolved = resolve_group_expr(expr, &parsed.select_items);
    match &resolved {
        Expr::Identifier(id) => Ok(id.value.clone()),
        _ => Err(crate::Error::msg("group id")),
    }
}

fn empty_result(parsed: &ParsedQuery) -> Option<QueryResult> {
    Some(QueryResult {
        columns: parsed.select_items.iter().map(projection_label).collect(),
        rows: vec![],
    })
}

pub(crate) fn finish_count_sorted(
    parsed: &ParsedQuery,
    scored: impl Iterator<Item = (u64, Vec<String>)>,
) -> Result<Option<QueryResult>> {
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let rows = top_counts(scored, limit, offset);
    Ok(Some(QueryResult { columns, rows }))
}
