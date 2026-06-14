mod aggregate;
mod agg_heap;
mod agg_topk;
mod column_slice;
mod fast_agg;
mod fast_q29;
mod filter;
mod mask_util;
mod filter_fast;
mod group;
mod group_direct;
mod group_fused_q11;
mod group_fused_q22;
mod group_fused_q23;
mod group_fused_q29;
mod group_columnar;
mod group_fused_q36;
mod group_fused_q41;
mod group_fused_q40;
mod group_fused_q43;
mod group_near_unique;
mod group_int;
mod group_fused;
mod group_referrer;
mod group_sorted;
mod group_utf8;
mod having;
mod simd_count;
mod simd_scan;
mod topk;
mod utf8_arena;
mod scan;
mod scan_fast;

use crate::sql::{parse_query, projection_label};
use crate::storage::ColumnData;
use crate::storage::Database;
use crate::Result;

pub use group::execute_grouped;
pub use scan::execute_scan;

use aggregate::eval_global_select;
use fast_agg::try_execute_global;
use fast_q29::try_execute_q29;
use crate::expr::{format_date_days, format_timestamp_micros};

use filter::build_filter_mask;

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn col_key(col: &ColumnData, i: usize) -> String {
    match col {
        ColumnData::Int64(v) => v[i].to_string(),
        ColumnData::Int32(v) => v[i].to_string(),
        ColumnData::Int16(v) => v[i].to_string(),
        ColumnData::Date(v) => format_date_days(v[i]),
        ColumnData::Timestamp(v) => format_timestamp_micros(v[i]),
        ColumnData::Utf8(v) => v[i].to_string(),
    }
}

pub fn execute(db: &mut Database, sql: &str) -> Result<QueryResult> {
    let parsed = parse_query(sql)?;
    if parsed.from_table != "hits" {
        return Err(crate::Error::msg(format!(
            "unknown table {}",
            parsed.from_table
        )));
    }

    if !parsed.group_by.is_empty() {
        return group::execute_grouped(db, &parsed);
    }

    let is_scan = parsed.select_all
        || parsed.order_by.iter().any(|_| true)
        || parsed.limit.is_some()
        || parsed.offset.is_some()
        || parsed
            .select_items
            .iter()
            .any(|p| matches!(p.kind, crate::sql::SelectItemKind::Column(_) | crate::sql::SelectItemKind::Other(_)));

    if is_scan {
        return scan::execute_scan(db, &parsed);
    }

    let table = db.open_table_for_query("hits", &parsed)?;
    let row_count = table.row_count() as usize;

    if let Some(result) = try_execute_q29(&table, &parsed, row_count)? {
        return Ok(result);
    }

    if let Some(result) = try_execute_global(&table, &parsed, row_count)? {
        return Ok(result);
    }

    let mask = build_filter_mask(&table, parsed.where_expr.as_ref(), row_count)?;

    let mut columns = Vec::new();
    let mut values = Vec::new();
    for proj in &parsed.select_items {
        let (_name, val) = eval_global_select(&table, &proj.kind, &mask)?;
        columns.push(projection_label(proj));
        values.push(val);
    }
    Ok(QueryResult {
        columns,
        rows: vec![values],
    })
}
