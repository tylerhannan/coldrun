//! Fast GROUP BY for integer-only keys (no per-row String allocation).

use ahash::AHashMap;

use sqlparser::ast::{BinaryOperator, DateTimeField, Expr, Value};

use crate::expr::format_date_days;
use crate::sql::{expr_column_name, projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::aggregate::AggState;
use super::filter::build_filter_mask;
use super::mask_util::{mask_is_sparse, selected_indices};
use super::group::{
    eval_proj_at_row, is_aggregate, is_group_key_proj, resolve_group_expr, sort_rows,
};
use super::having::having_can_match;
use super::topk::truncate_to_top_k;
use super::QueryResult;

const MAX_PACKED_KEYS: usize = 4;

struct IntGroupSpec {
    exprs: Vec<Expr>,
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
    let selected = mask.iter().filter(|&&b| b).count() as u64;
    if let Some(having) = &parsed.having {
        if !having_can_match(having, selected.max(1)) {
            let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
            return Ok(Some(QueryResult {
                columns,
                rows: vec![],
            }));
        }
    }

    let mut groups: AHashMap<u128, GroupBucket> =
        AHashMap::with_capacity((selected as usize / 8).max(16));

    let row_iter: Vec<usize> = if mask_is_sparse(&mask) {
        selected_indices(&mask)
    } else {
        (0..row_count).filter(|&i| mask[i]).collect()
    };

    for i in row_iter {
        let key = pack_key(table, &spec.exprs, i)?;
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
    let mut exprs = Vec::with_capacity(parsed.group_by.len());
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        if !is_int_group_expr(table, &resolved)? {
            return Ok(None);
        }
        exprs.push(resolved);
    }
    Ok(Some(IntGroupSpec { exprs }))
}

fn is_int_group_expr(table: &Table, expr: &Expr) -> Result<bool> {
    match expr {
        Expr::Identifier(id) => {
            let ty = table
                .column_type(&id.value)
                .ok_or_else(|| crate::Error::msg(format!("column {}", id.value)))?;
            Ok(matches!(
                ty,
                ColumnType::Int16 | ColumnType::Int32 | ColumnType::Int64 | ColumnType::Date
            ))
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Minus,
            right,
        } => {
            Ok(is_int_group_expr(table, left)?
                && matches!(&**right, Expr::Value(Value::Number(_, _))))
        }
        Expr::Value(Value::Number(_, _)) => Ok(true),
        Expr::Extract {
            field: DateTimeField::Minute,
            expr,
            ..
        } => {
            if let Expr::Identifier(id) = &**expr {
                Ok(table.column_type(&id.value) == Some(ColumnType::Timestamp))
            } else {
                Ok(false)
            }
        }
        _ => Ok(false),
    }
}

fn eval_int_group_key(table: &Table, expr: &Expr, row: usize) -> Result<i64> {
    match expr {
        Expr::Identifier(id) => {
            let col = table.column(&id.value)?;
            Ok(i64_key_at(&col, row))
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Minus,
            right,
        } => {
            let l = eval_int_group_key(table, left, row)?;
            let r = match &**right {
                Expr::Value(Value::Number(n, _)) => n
                    .parse::<i64>()
                    .map_err(|e| crate::Error::msg(format!("bad number: {e}")))?,
                _ => eval_int_group_key(table, right, row)?,
            };
            Ok(l - r)
        }
        Expr::Value(Value::Number(n, _)) => n
            .parse::<i64>()
            .map_err(|e| crate::Error::msg(format!("bad number: {e}"))),
        Expr::Extract {
            field: DateTimeField::Minute,
            expr,
            ..
        } => {
            let col = table.column(match &**expr {
                Expr::Identifier(id) => &id.value,
                _ => return Err(crate::Error::msg("extract col")),
            })?;
            let micros = match col {
                ColumnData::Timestamp(v) => v[row],
                _ => 0,
            };
            Ok(((micros / 1_000_000) / 60) % 60)
        }
        _ => Err(crate::Error::msg("int group key")),
    }
}

fn pack_key(table: &Table, exprs: &[Expr], row: usize) -> Result<u128> {
    match exprs.len() {
        1 => Ok(eval_int_group_key(table, &exprs[0], row)? as u128),
        2 => {
            let a = eval_int_group_key(table, &exprs[0], row)? as u64;
            let b = eval_int_group_key(table, &exprs[1], row)? as u64;
            Ok(((a as u128) << 64) | (b as u128))
        }
        n if n <= 4 => {
            let mut key = 0u128;
            for (i, expr) in exprs.iter().enumerate().take(4) {
                let v = eval_int_group_key(table, expr, row)? as u32;
                key |= (v as u128) << (32 * i);
            }
            Ok(key)
        }
        _ => Err(crate::Error::msg("too many group keys")),
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

fn format_packed_group_key(raw: &str, expr: &Expr, table: &Table) -> String {
    if let Some(name) = expr_column_name(expr) {
        if let Ok(col) = table.column(&name) {
            return match col {
                ColumnData::Date(_) => {
                    format_date_days(raw.parse::<i32>().unwrap_or(0))
                }
                _ => raw.to_string(),
            };
        }
    }
    raw.to_string()
}

fn unpack_key(key: u128, ncols: usize) -> Vec<String> {
    match ncols {
        1 => vec![(key as i64).to_string()],
        2 => {
            let a = (key >> 64) as i64;
            let b = key as i64;
            vec![a.to_string(), b.to_string()]
        }
        n @ 3..=4 => (0..n)
            .map(|i| ((key >> (32 * i)) & 0xFFFF_FFFF) as i32)
            .map(|v| v.to_string())
            .collect(),
        _ => vec![],
    }
}

fn finish_groups(
    table: &Table,
    parsed: &ParsedQuery,
    groups: AHashMap<u128, GroupBucket>,
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
                let gb_expr = &parsed.group_by[key_idx];
                let k = key.get(key_idx).cloned().unwrap_or_default();
                key_idx += 1;
                let k = format_packed_group_key(&k, gb_expr, table);
                if matches!(proj.kind, SelectItemKind::Other(_) | SelectItemKind::Column(_)) {
                    k
                } else {
                    let (_, s) = state.finish(&proj.kind, Some(k.as_str()), Some(table))?;
                    s
                }
            } else if matches!(proj.kind, SelectItemKind::Other(_)) {
                eval_proj_at_row(table, proj, bucket.sample_row)?
            } else {
                let (_, s) = state.finish(&proj.kind, None, Some(table))?;
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
