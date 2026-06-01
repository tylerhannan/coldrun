//! Q11: single utf8 key + COUNT(DISTINCT UserID).

use ahash::AHashMap;

use sqlparser::ast::Expr;

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, ColumnType, Table};
use crate::Result;

use super::group::resolve_group_expr;
use super::group_fused::{build_mask, finish_count_sorted_legacy};
use super::mask_util::for_each_selected;
use super::utf8_arena::Utf8Intern;
use super::QueryResult;

pub fn try_fused_utf8_one_distinct(
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
    if table.column_type(&id.value) != Some(ColumnType::Utf8) {
        return Ok(None);
    }
    let mut distinct_user = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::CountDistinct(e) => {
                if crate::sql::expr_column_name(e).as_deref() != Some("UserID") {
                    return Ok(None);
                }
                distinct_user = true;
            }
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !distinct_user || parsed.select_items.len() != 2 {
        return Ok(None);
    }

    let ColumnData::Utf8(keys) = table.column(&id.value)? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(Some(QueryResult {
            columns: parsed.select_items.iter().map(projection_label).collect(),
            rows: vec![],
        }));
    };

    let mut intern = Utf8Intern::with_capacity(512);
    let mut groups: AHashMap<u32, AHashMap<i64, ()>> = AHashMap::with_capacity(512);

    for_each_selected(&mask, row_count, |i| {
        let kid = intern.intern(&keys[i]);
        groups.entry(kid).or_default().insert(users[i], ());
    });

    let out = groups.into_iter().map(|(kid, set)| {
        let u = set.len() as u64;
        (u, vec![intern.get(kid).to_string(), u.to_string()])
    });
    finish_count_sorted_legacy(parsed, out)
}
