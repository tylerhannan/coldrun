//! GROUP BY on one or two Utf8 columns without per-row `eval_group_key`.

use ahash::AHashMap;

use sqlparser::ast::{Expr, Value};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::aggregate::AggState;
use super::filter::build_filter_mask;
use super::group::{
    eval_proj_at_row, is_aggregate, is_group_key_proj, resolve_group_expr, sort_rows,
};
use super::group_int::apply_limit_offset;
use super::having::having_can_match;
use super::mask_util::{mask_is_sparse, selected_indices};
use super::topk::truncate_to_top_k;
use super::QueryResult;

const MAX_UTF8_KEYS: usize = 2;

struct Utf8GroupSpec {
    names: Vec<String>,
}

pub fn try_execute_grouped_utf8(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    let spec = match plan_utf8_group(table, parsed)? {
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

    let col_refs: Vec<&ColumnData> = spec.names.iter().map(|n| table.column(n)).collect::<Result<_>>()?;

    let mut groups: AHashMap<String, GroupBucket> =
        AHashMap::with_capacity((selected as usize / 8).max(16));

    let row_iter: Vec<usize> = if mask_is_sparse(&mask) {
        selected_indices(&mask)
    } else {
        (0..row_count).filter(|&i| mask[i]).collect()
    };

    for i in row_iter {
        let (map_key, key_parts) = key_parts(&col_refs, i);
        let bucket = groups.entry(map_key.clone()).or_insert_with(|| GroupBucket {
            states: parsed
                .select_items
                .iter()
                .map(|_| AggState::default())
                .collect(),
            sample_row: i,
            key_parts,
        });

        for (state, proj) in bucket.states.iter_mut().zip(parsed.select_items.iter()) {
            if is_aggregate(&proj.kind) {
                state.update(table, &proj.kind, i)?;
            }
        }
    }

    finish_utf8_groups(table, parsed, groups)
}

struct GroupBucket {
    states: Vec<AggState>,
    sample_row: usize,
    key_parts: Vec<String>,
}

fn plan_utf8_group(table: &Table, parsed: &ParsedQuery) -> Result<Option<Utf8GroupSpec>> {
    if parsed.group_by.is_empty() || parsed.group_by.len() > MAX_UTF8_KEYS {
        return Ok(None);
    }
    let mut names = Vec::with_capacity(parsed.group_by.len());
    for expr in &parsed.group_by {
        let resolved = resolve_group_expr(expr, &parsed.select_items);
        match &resolved {
            Expr::Identifier(id) => {
                let ty = table
                    .column_type(&id.value)
                    .ok_or_else(|| crate::Error::msg(format!("column {}", id.value)))?;
                if ty != ColumnType::Utf8 {
                    return Ok(None);
                }
                names.push(id.value.clone());
            }
            Expr::Value(Value::Number(n, _)) if n == "1" => {
                // Q35 `GROUP BY 1, URL` — constant does not partition groups
            }
            _ => return Ok(None),
        }
    }
    if names.is_empty() {
        return Ok(None);
    }
    Ok(Some(Utf8GroupSpec { names }))
}

fn key_parts(cols: &[&ColumnData], row: usize) -> (String, Vec<String>) {
    match cols.len() {
        1 => {
            let s = match cols[0] {
                ColumnData::Utf8(v) => v[row].to_string(),
                _ => String::new(),
            };
            (s.clone(), vec![s])
        }
        2 => {
            let a = match cols[0] {
                ColumnData::Utf8(v) => v[row].to_string(),
                _ => String::new(),
            };
            let b = match cols[1] {
                ColumnData::Utf8(v) => v[row].to_string(),
                _ => String::new(),
            };
            (format!("{a}\0{b}"), vec![a, b])
        }
        _ => (String::new(), vec![]),
    }
}

fn finish_utf8_groups(
    table: &Table,
    parsed: &ParsedQuery,
    groups: AHashMap<String, GroupBucket>,
) -> Result<Option<QueryResult>> {
    let column_names: Vec<String> = parsed
        .select_items
        .iter()
        .map(projection_label)
        .collect();

    let mut rows = Vec::new();
    for (_k, bucket) in groups {
        if let Some(having) = &parsed.having {
            let pass = bucket
                .states
                .iter()
                .any(|s| s.passes_having(having).unwrap_or(false));
            if !pass {
                continue;
            }
        }

        let mut row = Vec::with_capacity(parsed.select_items.len());
        let mut key_idx = 0;
        for (proj, state) in parsed.select_items.iter().zip(bucket.states.iter()) {
            let val = if is_group_key_proj(proj, &parsed.group_by) {
                let v = bucket.key_parts.get(key_idx).map(|s| s.as_str());
                key_idx += 1;
                if matches!(proj.kind, SelectItemKind::Other(_) | SelectItemKind::Column(_)) {
                    eval_proj_at_row(table, proj, bucket.sample_row)?
                } else {
                    let (_, s) = state.finish(&proj.kind, v, Some(&table))?;
                    s
                }
            } else if matches!(proj.kind, SelectItemKind::Other(_)) {
                eval_proj_at_row(table, proj, bucket.sample_row)?
            } else {
                let (_, s) = state.finish(&proj.kind, None, Some(&table))?;
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
