use ahash::AHashMap;

use sqlparser::ast::Expr;

use crate::expr::{eval_group_key, eval_i64, eval_string};
use crate::sql::{projection_label, ParsedQuery, SelectItemKind, SelectProjection};
use crate::storage::Database;
use crate::Result;

use super::aggregate::AggState;
use super::filter::build_filter_mask;
use super::mask_util::{mask_is_sparse, selected_indices};
use super::group_int::{apply_limit_offset, try_execute_grouped_int};
use super::topk::truncate_to_top_k;
use super::QueryResult;

struct GroupBucket {
    states: Vec<AggState>,
    sample_row: usize,
}

pub fn execute_grouped(db: &Database, parsed: &ParsedQuery) -> Result<QueryResult> {
    let table = db.open_table_for_query("hits", parsed)?;
    let row_count = table.row_count() as usize;

    if let Some(result) = super::group_near_unique::try_execute_near_unique(
        &table, parsed, row_count,
    )? {
        return Ok(result);
    }

    if let Some(result) = super::group_direct::try_execute_group_direct(
        &table, parsed, row_count,
    )? {
        return Ok(result);
    }

    if let Some(result) = super::group_sorted::try_execute_group_sorted(
        &table, parsed, row_count,
    )? {
        return Ok(result);
    }

    if let Some(result) = super::group_fused::try_execute_group_fused(
        &table, parsed, row_count,
    )? {
        return Ok(result);
    }

    if let Some(result) = super::group_referrer::try_execute_grouped_referrer(
        &table, parsed, row_count,
    )? {
        return Ok(result);
    }

    if let Some(result) = super::group_utf8::try_execute_grouped_utf8(
        &table, parsed, row_count,
    )? {
        return Ok(result);
    }

    if let Some(result) = try_execute_grouped_int(&table, parsed, row_count)? {
        return Ok(result);
    }

    let mask = build_filter_mask(&table, parsed.where_expr.as_ref(), row_count)?;

    let selected = mask.iter().filter(|&&b| b).count();
    if let Some(having) = &parsed.having {
        if !super::having::having_can_match(having, selected.max(1) as u64) {
            let column_names: Vec<String> = parsed
                .select_items
                .iter()
                .map(crate::sql::projection_label)
                .collect();
            return Ok(QueryResult {
                columns: column_names,
                rows: vec![],
            });
        }
    }
    let mut groups: AHashMap<Vec<String>, GroupBucket> =
        AHashMap::with_capacity((selected / 8).max(16));

    let row_iter: Vec<usize> = if mask_is_sparse(&mask) {
        selected_indices(&mask)
    } else {
        (0..row_count).filter(|&i| mask[i]).collect()
    };

    for i in row_iter {
        let mut key = Vec::with_capacity(parsed.group_by.len());
        for expr in &parsed.group_by {
            let resolved = resolve_group_expr(expr, &parsed.select_items);
            key.push(eval_group_key(&table, &resolved, i)?);
        }

        let bucket = groups.entry(key).or_insert_with(|| GroupBucket {
            states: parsed
                .select_items
                .iter()
                .map(|_| AggState::default())
                .collect(),
            sample_row: i,
        });

        for (state, proj) in bucket.states.iter_mut().zip(parsed.select_items.iter()) {
            if is_aggregate(&proj.kind) {
                state.update(&table, &proj.kind, i)?;
            }
        }
    }

    let column_names: Vec<String> = parsed
        .select_items
        .iter()
        .map(projection_label)
        .collect();

    let mut rows = Vec::new();
    for (key, bucket) in groups {
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
                let v = key.get(key_idx).map(|s| s.as_str());
                key_idx += 1;
                if matches!(proj.kind, SelectItemKind::Other(_) | SelectItemKind::Column(_)) {
                    eval_proj_at_row(&table, proj, bucket.sample_row)?
                } else {
                    let (_, s) = state.finish(&proj.kind, v)?;
                    s
                }
            } else if matches!(proj.kind, SelectItemKind::Other(_)) {
                eval_proj_at_row(&table, proj, bucket.sample_row)?
            } else {
                let (_, s) = state.finish(&proj.kind, None)?;
                s
            };
            row.push(val);
        }
        rows.push(row);
    }

    truncate_to_top_k(parsed, &column_names, &mut rows);
    sort_rows(&parsed, &column_names, &mut rows)?;
    apply_limit_offset(parsed, &mut rows);

    Ok(QueryResult {
        columns: column_names,
        rows,
    })
}

pub(crate) fn is_aggregate(kind: &SelectItemKind) -> bool {
    !matches!(kind, SelectItemKind::Column(_) | SelectItemKind::Other(_))
}

pub(crate) fn is_group_key_proj(proj: &SelectProjection, group_by: &[Expr]) -> bool {
    if let SelectItemKind::Column(e) = &proj.kind {
        return group_by.iter().any(|g| g == e);
    }
    if let Some(alias) = &proj.alias {
        return group_by.iter().any(|g| {
            if let Expr::Identifier(id) = g {
                id.value == *alias
            } else {
                false
            }
        });
    }
    false
}

pub(crate) fn resolve_group_expr(expr: &Expr, projections: &[SelectProjection]) -> Expr {
    if let Expr::Identifier(id) = expr {
        for p in projections {
            if p.alias.as_deref() == Some(&id.value) {
                return match &p.kind {
                    SelectItemKind::Column(e) | SelectItemKind::Other(e) => e.clone(),
                    _ => expr.clone(),
                };
            }
        }
    }
    expr.clone()
}

pub(crate) fn eval_proj_at_row(
    table: &crate::storage::Table,
    proj: &SelectProjection,
    row: usize,
) -> Result<String> {
    match &proj.kind {
        SelectItemKind::Column(e) | SelectItemKind::Other(e) => {
            if let Ok(s) = eval_string(table, e, row) {
                if !s.is_empty() {
                    return Ok(s);
                }
            }
            Ok(eval_i64(table, e, row)?.to_string())
        }
        _ => Err(crate::Error::msg("expected projection expr")),
    }
}

pub(crate) fn sort_rows(
    parsed: &crate::sql::ParsedQuery,
    columns: &[String],
    rows: &mut [Vec<String>],
) -> Result<()> {
    if parsed.order_by.is_empty() {
        return Ok(());
    }

    rows.sort_by(|a, b| {
        for (order_expr, desc) in &parsed.order_by {
            let col_idx = resolve_order_column(order_expr, columns, &parsed.select_items)
                .unwrap_or(0);
            let cmp = compare_cell(&a[col_idx], &b[col_idx]);
            let ord = if *desc { cmp.reverse() } else { cmp };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
    Ok(())
}

fn resolve_order_column(
    expr: &Expr,
    columns: &[String],
    projections: &[SelectProjection],
) -> Result<usize> {
    if let Expr::Identifier(ident) = expr {
        if let Some(i) = columns.iter().position(|c| c == &ident.value) {
            return Ok(i);
        }
    }

    if let Expr::Function(f) = expr {
        if f.name.to_string().to_uppercase() == "COUNT" {
            if let Some(i) = columns.iter().position(|c| c == "count()") {
                return Ok(i);
            }
        }
        if f.name.to_string().to_uppercase() == "DATE_TRUNC" {
            if let Some(i) = columns.iter().position(|c| c == "M") {
                return Ok(i);
            }
        }
    }

    if let Some(name) = crate::sql::expr_column_name(expr) {
        if let Some(i) = columns.iter().position(|c| c == &name) {
            return Ok(i);
        }
    }

    for (i, p) in projections.iter().enumerate() {
        if let Some(alias) = &p.alias {
            if let Expr::Identifier(ident) = expr {
                if alias == &ident.value {
                    return Ok(i);
                }
            }
        }
    }

    Err(crate::Error::msg(format!("ORDER BY not resolved: {expr}")))
}

fn compare_cell(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}
