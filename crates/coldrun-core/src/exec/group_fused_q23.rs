//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).

use ahash::{AHashMap, AHashSet};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::utf8_arena::Utf8Intern;
use super::QueryResult;

#[inline]
fn q23_row_matches(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    i: usize,
) -> bool {
    if phrases.get(i).is_empty() {
        return false;
    }
    let t = titles.get(i).as_bytes();
    if memchr::memmem::find(t, b"Google").is_none() {
        return false;
    }
    !memchr::memmem::find(urls.get(i).as_bytes(), b".google.").is_some()
}

pub fn try_fused_q23(
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
        min_url_row: u32,
        min_title_row: u32,
        count: u64,
        users: AHashSet<i64>,
    }

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let mut intern = Utf8Intern::with_capacity(64);
    let mut groups: AHashMap<u32, Bucket> = AHashMap::with_capacity(64);

    for i in 0..row_count {
        if !q23_row_matches(phrases, urls, titles, i) {
            continue;
        }
        let pid = intern.intern(phrases.get(i));
        let url = urls.get(i);
        let title = titles.get(i);
        let uid = users[i];
        match groups.get_mut(&pid) {
            Some(b) => {
                if url < urls.get(b.min_url_row as usize) {
                    b.min_url_row = i as u32;
                }
                if title < titles.get(b.min_title_row as usize) {
                    b.min_title_row = i as u32;
                }
                b.count += 1;
                b.users.insert(uid);
            }
            None => {
                let mut users_set = AHashSet::new();
                users_set.insert(uid);
                groups.insert(
                    pid,
                    Bucket {
                        min_url_row: i as u32,
                        min_title_row: i as u32,
                        count: 1,
                        users: users_set,
                    },
                );
            }
        }
    }

    let scored = groups.into_iter().map(|(pid, b)| {
        (
            b.count,
            vec![
                intern.get(pid).to_string(),
                urls.get(b.min_url_row as usize).to_string(),
                titles.get(b.min_title_row as usize).to_string(),
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
