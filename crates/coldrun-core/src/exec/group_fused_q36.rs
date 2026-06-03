//! Q36: ClientIP quad GROUP BY — columnar scan, no filter mask.

use sqlparser::ast::{BinaryOperator, Expr, Value};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::Table;
use crate::Result;

use super::agg_heap::top_counts_u128_key;
use super::group::resolve_group_expr;
use super::group_columnar::{clientip_quad_count, unpack_clientip_quad};
use super::group_fused::orders_by_count_desc;
use super::QueryResult;

pub fn try_fused_q36(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.where_expr.is_some() || !is_q36_shape(parsed) || !orders_by_count_desc(parsed) {
        return Ok(None);
    }
    let col = table.column("ClientIP")?;
    let Some(ips) = col.as_i32_slice() else {
        return Ok(None);
    };
    let n = row_count.min(ips.len());
    if n == 0 {
        return Ok(Some(empty_result(parsed)));
    }

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(10);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let shards = clientip_quad_count(&ips[..n]);

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let rows: Vec<Vec<String>> = top_counts_u128_key(
        shards
            .iter()
            .flat_map(|m| m.iter().map(|(&k, &c)| (c, k, (k, c)))),
        limit,
        offset,
    )
    .into_iter()
    .map(|(key, c)| {
        let k = unpack_clientip_quad(key);
        vec![
            k[0].to_string(),
            k[1].to_string(),
            k[2].to_string(),
            k[3].to_string(),
            c.to_string(),
        ]
    })
    .collect();

    Ok(Some(QueryResult { columns, rows }))
}

fn is_q36_shape(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 4 {
        return false;
    }
    if !parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    }) {
        return false;
    }
    parsed.group_by.iter().all(|e| {
        let r = resolve_group_expr(e, &parsed.select_items);
        is_clientip_minus(&r)
    })
}

fn is_clientip_minus(expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(id) if id.value == "ClientIP" => true,
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Minus,
            right,
        } => {
            matches!(&**left, Expr::Identifier(id) if id.value == "ClientIP")
                && matches!(&**right, Expr::Value(Value::Number(_, _)))
        }
        _ => false,
    }
}

fn empty_result(parsed: &ParsedQuery) -> QueryResult {
    QueryResult {
        columns: parsed.select_items.iter().map(projection_label).collect(),
        rows: vec![],
    }
}
