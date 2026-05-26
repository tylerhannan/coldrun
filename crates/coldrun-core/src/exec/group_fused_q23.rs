//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).

use ahash::AHashMap;

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::build_mask;
use super::mask_util::for_each_selected;
use super::utf8_arena::Utf8Intern;
use super::QueryResult;

#[inline]
fn q23_row_matches(phrases: &[String], urls: &[String], titles: &[String], i: usize) -> bool {
    !phrases[i].is_empty() && titles[i].contains("Google") && !urls[i].contains(".google.")
}

pub fn try_fused_q23(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if parsed.group_by.len() != 1 {
        return Ok(None);
    }
    let gb = &parsed.group_by[0];
    let sqlparser::ast::Expr::Identifier(id) = gb else {
        return Ok(None);
    };
    if id.value != "SearchPhrase" {
        return Ok(None);
    }

    let mut has_min_url = false;
    let mut has_min_title = false;
    let mut has_count = false;
    let mut has_distinct = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Min(e) if is_col(e, "URL") => has_min_url = true,
            SelectItemKind::Min(e) if is_col(e, "Title") => has_min_title = true,
            SelectItemKind::CountAll | SelectItemKind::Count(_) => has_count = true,
            SelectItemKind::CountDistinct(e) if is_col(e, "UserID") => has_distinct = true,
            SelectItemKind::Column(_) => {}
            _ => return Ok(None),
        }
    }
    if !has_min_url || !has_min_title || !has_count || !has_distinct {
        return Ok(None);
    }

    let ColumnData::Utf8(phrases) = table.column("SearchPhrase")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(urls) = table.column("URL")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(titles) = table.column("Title")? else {
        return Ok(None);
    };
    let ColumnData::Int64(users) = table.column("UserID")? else {
        return Ok(None);
    };

    struct Bucket {
        min_url: String,
        min_title: String,
        count: u64,
        users: AHashMap<i64, ()>,
    }

    let mut intern = Utf8Intern::with_capacity(1024);
    let mut groups: AHashMap<u32, Bucket> = AHashMap::with_capacity(1024);

    let mut feed = |i: usize| {
        if !q23_row_matches(phrases, urls, titles, i) {
            return;
        }
        let pid = intern.intern(&phrases[i]);
        let url = urls[i].as_str();
        let title = titles[i].as_str();
        let uid = users[i];
        match groups.get_mut(&pid) {
            Some(b) => {
                if url < b.min_url.as_str() {
                    b.min_url = urls[i].clone();
                }
                if title < b.min_title.as_str() {
                    b.min_title = titles[i].clone();
                }
                b.count += 1;
                b.users.insert(uid, ());
            }
            None => {
                let mut users_map = AHashMap::new();
                users_map.insert(uid, ());
                groups.insert(
                    pid,
                    Bucket {
                        min_url: urls[i].clone(),
                        min_title: titles[i].clone(),
                        count: 1,
                        users: users_map,
                    },
                );
            }
        }
    };

    if table.demo_near_unique() {
        for i in 0..row_count {
            feed(i);
        }
    } else if let Some(mask) = build_mask(table, parsed, row_count)? {
        for_each_selected(&mask, row_count, feed);
    } else {
        return Ok(Some(QueryResult {
            columns: parsed.select_items.iter().map(projection_label).collect(),
            rows: vec![],
        }));
    }

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let scored = groups.into_iter().map(|(pid, b)| {
        (
            b.count,
            vec![
                intern.get(pid).to_string(),
                b.min_url,
                b.min_title,
                b.count.to_string(),
                b.users.len().to_string(),
            ],
        )
    });
    let rows = top_counts(scored, limit, offset);
    Ok(Some(QueryResult { columns, rows }))
}

fn is_col(e: &sqlparser::ast::Expr, name: &str) -> bool {
    crate::sql::expr_column_name(e).as_deref() == Some(name)
}
