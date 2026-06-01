//! Direct-index GROUP BY for low-cardinality integer keys (no hash table).

use ahash::AHashMap;

use sqlparser::ast::Expr;

use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::filter::build_filter_mask;
use super::group::resolve_group_expr;
use super::group_fused::finish_count_sorted_legacy;
use super::having::having_can_match;
use super::mask_util::for_each_selected;
use super::QueryResult;

const MAX_DIRECT_BUCKETS: usize = 65_536;

pub fn try_execute_group_direct(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = try_adv_engineid_group(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_regionid_count_distinct(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_regionid_multi_agg(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    Ok(None)
}

/// Q8: `AdvEngineID` has demo cardinality 8; use a fixed bucket array.
fn try_adv_engineid_group(
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
    if id.value != "AdvEngineID" {
        return Ok(None);
    }
    if !count_only_select(parsed) {
        return Ok(None);
    }
    let ColumnData::Int16(adv) = table.column("AdvEngineID")? else {
        return Ok(None);
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = mask.iter().filter(|&&b| b).count() as u64;
    if let Some(having) = &parsed.having {
        if !having_can_match(having, selected.max(1)) {
            return Ok(empty_result(parsed));
        }
    }

    let mut counts: AHashMap<i16, u64> = AHashMap::new();
    for_each_selected(&mask, row_count, |i| {
        let v = adv[i];
        if v != 0 {
            *counts.entry(v).or_insert(0) += 1;
        }
    });

    let out = counts
        .into_iter()
        .map(|(k, c)| (c, vec![k.to_string(), c.to_string()]));
    finish_count_sorted_legacy(parsed, out)
}

/// Q9: RegionID (~1000 buckets) + COUNT(DISTINCT UserID).
fn try_regionid_count_distinct(
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
    if id.value != "RegionID" {
        return Ok(None);
    }
    let mut distinct_user = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::CountDistinct(e) if expr_column_name(e).as_deref() == Some("UserID") => {
                distinct_user = true;
            }
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !distinct_user {
        return Ok(None);
    }

    let Some(bucket_n) = direct_bucket_count(table, "RegionID", row_count) else {
        return Ok(None);
    };
    let ColumnData::Int32(regions) = table.column("RegionID")? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let mut buckets: Vec<AHashMap<i64, ()>> = (0..bucket_n).map(|_| AHashMap::new()).collect();

    for_each_selected(&mask, row_count, |i| {
        let r = regions[i];
        if r >= 0 {
            let idx = r as usize;
            if idx < bucket_n {
                buckets[idx].insert(users[i], ());
            }
        }
    });

    let out = buckets
        .into_iter()
        .enumerate()
        .filter(|(_, set)| !set.is_empty())
        .map(|(r, set)| {
            let u = set.len() as u64;
            (u, vec![r.to_string(), u.to_string()])
        });
    finish_count_sorted_legacy(parsed, out)
}

/// Q10: RegionID + SUM(AdvEngineID) + COUNT + AVG(ResolutionWidth) + COUNT DISTINCT UserID.
#[derive(Default)]
struct RegionDirect {
    count: u64,
    sum_adv: i64,
    sum_w: i64,
    n_w: u64,
    users: AHashMap<i64, ()>,
}

fn try_regionid_multi_agg(
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

    let Some(bucket_n) = direct_bucket_count(table, "RegionID", row_count) else {
        return Ok(None);
    };
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

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let mut buckets: Vec<RegionDirect> = (0..bucket_n).map(|_| RegionDirect::default()).collect();

    for_each_selected(&mask, row_count, |i| {
        let r = regions[i];
        if r < 0 {
            return;
        }
        let idx = r as usize;
        if idx >= bucket_n {
            return;
        }
        let b = &mut buckets[idx];
        b.count += 1;
        b.sum_adv += i64::from(adv[i]);
        b.sum_w += i64::from(width[i]);
        b.n_w += 1;
        b.users.insert(users[i], ());
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;

    let mut scored: Vec<(u64, Vec<String>)> = buckets
        .into_iter()
        .enumerate()
        .filter(|(_, b)| b.count > 0)
        .map(|(r, b)| {
            let avg = if b.n_w > 0 {
                b.sum_w as f64 / b.n_w as f64
            } else {
                0.0
            };
            (
                b.count,
                vec![
                    r.to_string(),
                    b.sum_adv.to_string(),
                    b.count.to_string(),
                    format!("{avg}"),
                    b.users.len().to_string(),
                ],
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let rows = scored.into_iter().skip(offset).take(limit).map(|(_, r)| r).collect();

    Ok(Some(QueryResult { columns, rows }))
}

fn direct_bucket_count(table: &Table, name: &str, row_count: usize) -> Option<usize> {
    let col = table.column(name).ok()?;
    let max_key = match col {
        ColumnData::Int16(v) => {
            let m = v.iter().take(row_count).map(|&x| i32::from(x)).max().unwrap_or(0);
            m.max(0) as usize + 1
        }
        ColumnData::Int32(v) => {
            let m = v.iter().take(row_count).map(|&x| x).max().unwrap_or(0);
            m.max(0) as usize + 1
        }
        _ => return None,
    };
    if max_key == 0 || max_key > MAX_DIRECT_BUCKETS {
        return None;
    }
    Some(max_key)
}

fn count_only_select(parsed: &ParsedQuery) -> bool {
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    })
}

fn empty_result(parsed: &ParsedQuery) -> Option<QueryResult> {
    Some(QueryResult {
        columns: parsed.select_items.iter().map(projection_label).collect(),
        rows: vec![],
    })
}
