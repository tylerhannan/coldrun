//! Fast GROUP BY for integer-only keys (no per-row String allocation).

use std::collections::HashMap;

use sqlparser::ast::Expr;

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::aggregate::AggState;
use super::filter::build_filter_mask;
use super::group::{
    eval_proj_at_row, is_aggregate, is_group_key_proj, resolve_group_expr, sort_rows,
};
use super::topk::truncate_to_top_k;
use super::QueryResult;

const MAX_PACKED_KEYS: usize = 2;

struct IntGroupSpec {
    col_names: Vec<String>,
}

pub fn try_execute_grouped_int(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let spec = match plan_int_group(table, parsed)? {
        Some(s) => s,
        None => return Ok(None),
    };

    let mask = build_filter_mask(table, parsed.where_expr.as_ref(), row_count)?;
    let mut groups: HashMap<u128, GroupBucket> = HashMap::new();

    let col_refs: Vec<&ColumnData> = spec
        .col_names
        .iter()
        .map(|n| table.column(n))
        .collect::<Result<_>>()?;

    for i in 0..row_count {
        if !mask[i] {
            continue;
        }
        let key = pack_key(&col_refs, i);
        let bucket = groups.entry(key).or_insert_with(|| GroupBucket {
            states: parsed
                .select_items
                .iter()
                .map(|_| AggState::default())
                .collect(),
            sample_row: i,
            key,
        });

        for (state, proj) in bucket.states.iter_mut().zip(parsed.select_items.iter()) {
            if is_aggregate(&proj.kind) {
                state.update(table, &proj.kind, i)?;
            }
        }
    }

    finish_groups(table, parsed, groups)
}

struct GroupBucket {
    states: Vec<AggState>,
    sample_row: usize,
    key: u128,
}

fn plan_int_group(table: &Table, parsed: &ParsedQuery) -> Result<Option<IntGroupSpec>> {
    if parsed.group_by.is_empty() || parsed.group_by.len() > MAX_PACKED_KEYS {
        return Ok(None);
    }
    let mut col_names = Vec::with_capacity(parsed.group_by.len());
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        let name = match &resolved {
            Expr::Identifier(id) => id.value.clone(),
            _ => return Ok(None),
        };
        let ty = table
            .column_type(&name)
            .ok_or_else(|| crate::Error::msg(format!("column {name}")))?;
        if !matches!(
            ty,
            ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date
        ) {
            return Ok(None);
        }
        col_names.push(name);
    }
    Ok(Some(IntGroupSpec { col_names }))
}

fn pack_key(cols: &[&ColumnData], row: usize) -> u128 {
    match cols.len() {
        1 => i64_key_at(cols[0], row) as u128,
        2 => {
            let a = i64_key_at(cols[0], row) as u64;
            let b = i64_key_at(cols[1], row) as u64;
            ((a as u128) << 64) | (b as u128)
        }
        _ => 0,
    }
}

fn i64_key_at(col: &ColumnData, row: usize) -> i64 {
    match col {
        ColumnData::Int64(v) => v[row],
        ColumnData::Int32(v) => i64::from(v[row]),
        ColumnData::Int16(v) => i64::from(v[row]),
        ColumnData::Date(v) => i64::from(v[row]),
        _ => 0,
    }
}

fn unpack_key(key: u128, ncols: usize) -> Vec<String> {
    match ncols {
        1 => vec![(key as i64).to_string()],
        2 => {
            let a = (key >> 64) as i64;
            let b = key as i64;
            vec![a.to_string(), b.to_string()]
        }
        _ => vec![],
    }
}

fn finish_groups(
    table: &Table,
    parsed: &ParsedQuery,
    groups: HashMap<u128, GroupBucket>,
) -> Result<Option<QueryResult>> {
    let ncols = parsed.group_by.len();
    let column_names: Vec<String> = parsed
        .select_items
        .iter()
        .map(projection_label)
        .collect();

    let mut rows = Vec::new();
    for (_packed, bucket) in groups {
        if let Some(having) = &parsed.having {
            let pass = bucket
                .states
                .iter()
                .any(|s| s.passes_having(having).unwrap_or(false));
            if !pass {
                continue;
            }
        }

        let key = unpack_key(bucket.key, ncols);
        let mut row = Vec::with_capacity(parsed.select_items.len());
        let mut key_idx = 0;
        for (proj, state) in parsed.select_items.iter().zip(bucket.states.iter()) {
            let val = if is_group_key_proj(proj, &parsed.group_by) {
                let v = key.get(key_idx).map(|s| s.as_str());
                key_idx += 1;
                if matches!(proj.kind, SelectItemKind::Other(_) | SelectItemKind::Column(_)) {
                    eval_proj_at_row(table, proj, bucket.sample_row)?
                } else {
                    let (_, s) = state.finish(&proj.kind, v)?;
                    s
                }
            } else if matches!(proj.kind, SelectItemKind::Other(_)) {
                eval_proj_at_row(table, proj, bucket.sample_row)?
            } else {
                let (_, s) = state.finish(&proj.kind, None)?;
                s
            };
            row.push(val);
        }
        rows.push(row);
    }

    truncate_to_top_k(parsed, &column_names, &mut rows);
    sort_rows(parsed, &column_names, &mut rows)?;
    apply_limit_offset(parsed, &mut rows);

    Ok(Some(QueryResult {
        columns: column_names,
        rows,
    }))
}

pub(crate) fn apply_limit_offset(parsed: &ParsedQuery, rows: &mut Vec<Vec<String>>) {
    if let Some(offset) = parsed.offset {
        let off = offset as usize;
        if off < rows.len() {
            rows.drain(0..off);
        } else {
            rows.clear();
        }
    }
    if let Some(limit) = parsed.limit {
        rows.truncate(limit as usize);
    }
}
