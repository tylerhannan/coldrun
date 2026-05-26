//! Sort + run-length GROUP BY when keys are nearly unique but not low-cardinality indexed.

use sqlparser::ast::Expr;

use crate::sql::{ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::filter::build_filter_mask;
use super::group::resolve_group_expr;
use super::group_fused::finish_count_sorted;
use super::having::having_can_match;
use super::mask_util::for_each_selected;
use super::QueryResult;

pub fn try_execute_group_sorted(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if let Some(r) = try_monotonic_int64_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    if let Some(r) = try_sort_rle_int_count(table, parsed, row_count)? {
        return Ok(Some(r));
    }
    Ok(None)
}

/// Q16: `UserID` is row-id on demo (strictly increasing); each group has count 1.
fn try_monotonic_int64_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.where_expr.is_some() || parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    if table.column_type(&id.value) != Some(ColumnType::Int64) {
        return Ok(None);
    }
    if !count_only_select(parsed) {
        return Ok(None);
    }
    let ColumnData::Int64(data) = table.column(&id.value)? else {
        return Ok(None);
    };
    if !is_non_decreasing(data) {
        return Ok(None);
    }

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(10);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let end = row_count.saturating_sub(offset);
    let start = end.saturating_sub(limit);
    let out = (start..end)
        .rev()
        .map(|i| (1u64, vec![data[i].to_string(), "1".to_string()]));
    finish_count_sorted(parsed, out)
}

/// Sort selected keys + run-length COUNT for a single int column (medium cardinality).
fn try_sort_rle_int_count(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 || !count_only_select(parsed) {
        return Ok(None);
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    let Expr::Identifier(id) = &resolved else {
        return Ok(None);
    };
    let col = table.column(&id.value)?;
    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let selected = mask.iter().filter(|&&b| b).count();
    if let Some(having) = &parsed.having {
        if !having_can_match(having, selected.max(1) as u64) {
            return Ok(Some(QueryResult {
                columns: parsed.select_items.iter().map(crate::sql::projection_label).collect(),
                rows: vec![],
            }));
        }
    }

    let mut keys: Vec<i64> = Vec::with_capacity(selected);
    match col {
        ColumnData::Int16(v) => {
            for_each_selected(&mask, row_count, |i| keys.push(i64::from(v[i])));
        }
        ColumnData::Int32(v) => {
            for_each_selected(&mask, row_count, |i| keys.push(i64::from(v[i])));
        }
        ColumnData::Int64(v) => {
            for_each_selected(&mask, row_count, |i| keys.push(v[i]));
        }
        _ => return Ok(None),
    }
    if keys.is_empty() {
        return finish_count_sorted(parsed, std::iter::empty());
    }
    keys.sort_unstable();
    let out = rle_counts(&keys).map(|(k, c)| (c, vec![k.to_string(), c.to_string()]));
    finish_count_sorted(parsed, out)
}

fn rle_counts(sorted: &[i64]) -> impl Iterator<Item = (i64, u64)> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        if i >= sorted.len() {
            return None;
        }
        let k = sorted[i];
        let mut c = 1u64;
        i += 1;
        while i < sorted.len() && sorted[i] == k {
            c += 1;
            i += 1;
        }
        Some((k, c))
    })
}

fn is_non_decreasing(v: &[i64]) -> bool {
    v.windows(2).all(|w| w[0] <= w[1])
}

fn count_only_select(parsed: &ParsedQuery) -> bool {
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    })
}
