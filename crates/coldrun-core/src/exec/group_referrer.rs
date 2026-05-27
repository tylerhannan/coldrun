//! Fast GROUP BY referer host (ClickBench Q29 pattern).

use ahash::AHashMap;

use sqlparser::ast::{Expr, Function, FunctionArg, FunctionArguments};

use crate::expr::referer_host;
use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
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

pub fn try_execute_grouped_referrer(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !is_referrer_host_group(parsed) {
        return Ok(None);
    }

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

    let referer = table.column("Referer")?;
    let ColumnData::Utf8(referer_col) = referer else {
        return Ok(None);
    };

    let row_iter: Vec<usize> = if mask_is_sparse(&mask) {
        selected_indices(&mask)
    } else {
        (0..row_count).filter(|&i| mask[i]).collect()
    };

    let mut groups: AHashMap<String, GroupBucket> =
        AHashMap::with_capacity((row_iter.len() / 8).max(16));

    for i in row_iter {
        let host = referer_host(&referer_col[i]);
        let bucket = groups.entry(host).or_insert_with(|| GroupBucket {
            states: parsed
                .select_items
                .iter()
                .map(|_| AggState::default())
                .collect(),
            sample_row: i,
        });
        for (state, proj) in bucket.states.iter_mut().zip(parsed.select_items.iter()) {
            if is_aggregate(&proj.kind) {
                state.update(table, &proj.kind, i)?;
            }
        }
    }

    finish_referrer_groups(table, parsed, groups)
}

struct GroupBucket {
    states: Vec<AggState>,
    sample_row: usize,
}

fn is_referrer_host_group(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 1 {
        return false;
    }
    let resolved = resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    match &resolved {
        Expr::Function(f) => is_referer_regexp(f),
        Expr::Identifier(id) => parsed.select_items.iter().any(|p| {
            p.alias.as_deref() == Some(&id.value) && matches!(&p.kind, SelectItemKind::Other(e) if is_referer_regexp_expr(e))
        }),
        _ => false,
    }
}

fn is_referer_regexp_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Function(f) => is_referer_regexp(f),
        _ => false,
    }
}

fn is_referer_regexp(f: &Function) -> bool {
    if f.name.to_string().to_uppercase() != "REGEXP_REPLACE" {
        return false;
    }
    let list = match &f.args {
        FunctionArguments::List(l) => &l.args,
        _ => return false,
    };
    let Some(FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(
        Expr::Identifier(id),
    ))) = list.first()
    else {
        return false;
    };
    id.value == "Referer"
}

fn finish_referrer_groups(
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
    for (host, bucket) in groups {
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
        for (proj, state) in parsed.select_items.iter().zip(bucket.states.iter()) {
            let val = if is_group_key_proj(proj, &parsed.group_by) {
                let v = Some(host.as_str());
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
