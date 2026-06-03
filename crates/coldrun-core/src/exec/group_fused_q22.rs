//! Q22: SearchPhrase + MIN(URL) + COUNT(*) on google URL filter.

use ahash::AHashMap;

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::build_mask;
use super::mask_util::for_each_selected;
use super::utf8_arena::Utf8Intern;
use super::QueryResult;

pub fn try_fused_q22(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let sqlparser::ast::Expr::Identifier(id) = &parsed.group_by[0] else {
        return Ok(None);
    };
    if id.value != "SearchPhrase" {
        return Ok(None);
    }

    let mut has_min_url = false;
    let mut has_count = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Min(e) if is_col(e, "URL") => has_min_url = true,
            SelectItemKind::CountAll | SelectItemKind::Count(_) => has_count = true,
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !has_min_url || !has_count || parsed.select_items.len() != 3 {
        return Ok(None);
    }

    let ColumnData::Utf8(phrases) = table.column("SearchPhrase")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(urls) = table.column("URL")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(Some(QueryResult {
            columns: parsed.select_items.iter().map(projection_label).collect(),
            rows: vec![],
        }));
    };

    struct Bucket {
        min_url_row: u32,
        count: u64,
    }

    let mut intern = Utf8Intern::with_capacity(512);
    let mut groups: AHashMap<u32, Bucket> = AHashMap::with_capacity(512);

    for_each_selected(&mask, row_count, |i| {
        let pid = intern.intern(phrases.get(i));
        let url = urls.get(i);
        match groups.get_mut(&pid) {
            Some(b) => {
                if url < urls.get(b.min_url_row as usize) {
                    b.min_url_row = i as u32;
                }
                b.count += 1;
            }
            None => {
                groups.insert(
                    pid,
                    Bucket {
                        min_url_row: i as u32,
                        count: 1,
                    },
                );
            }
        }
    });

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let scored = groups.into_iter().map(|(pid, b)| {
        (
            b.count,
            vec![
                intern.get(pid).to_string(),
                urls.get(b.min_url_row as usize).to_string(),
                b.count.to_string(),
            ],
        )
    });
    let rows = top_counts(scored, limit, offset);
    Ok(Some(QueryResult { columns, rows }))
}

fn is_col(e: &sqlparser::ast::Expr, name: &str) -> bool {
    crate::sql::expr_column_name(e).as_deref() == Some(name)
}
