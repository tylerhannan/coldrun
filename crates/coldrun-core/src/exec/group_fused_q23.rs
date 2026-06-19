//! Q23: SearchPhrase + MIN(URL) + MIN(Title) + COUNT + COUNT(DISTINCT UserID).

use ahash::{AHashMap, AHashSet};

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::hash_str;
use super::QueryResult;

const PARALLEL_THRESHOLD: usize = 250_000;

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

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let rows = if row_count >= PARALLEL_THRESHOLD {
        q23_parallel(phrases, urls, titles, users, row_count, limit, offset)
    } else {
        q23_serial(phrases, urls, titles, users, row_count, limit, offset)
    };

    Ok(Some(QueryResult { columns, rows }))
}

struct Bucket {
    min_url_row: u32,
    min_title_row: u32,
    count: u64,
    users: AHashSet<i64>,
}

fn q23_serial(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    users: &[i64],
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>> {
    let mut groups: AHashMap<u64, Bucket> = AHashMap::with_capacity(64);
    let mut phrase_by_hash: AHashMap<u64, String> = AHashMap::with_capacity(64);

    for i in 0..row_count {
        if !q23_row_matches(phrases, urls, titles, i) {
            continue;
        }
        let h = hash_str(phrases.get(i));
        phrase_by_hash.entry(h).or_insert_with(|| phrases.get(i).to_string());
        let url = urls.get(i);
        let title = titles.get(i);
        let uid = users[i];
        match groups.get_mut(&h) {
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
                    h,
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

    finish_q23(groups, phrase_by_hash, urls, titles, limit, offset)
}

fn q23_parallel(
    phrases: &crate::storage::Utf8Column,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    users: &[i64],
    row_count: usize,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>> {
    use rayon::prelude::*;

    let (groups, phrase_by_hash) = (0..row_count)
        .into_par_iter()
        .fold(
            || {
                (
                    AHashMap::<u64, Bucket>::with_capacity(32),
                    AHashMap::<u64, String>::with_capacity(32),
                )
            },
            |(mut groups, mut phrase_by_hash), i| {
                if !q23_row_matches(phrases, urls, titles, i) {
                    return (groups, phrase_by_hash);
                }
                let h = hash_str(phrases.get(i));
                phrase_by_hash
                    .entry(h)
                    .or_insert_with(|| phrases.get(i).to_string());
                let url = urls.get(i);
                let title = titles.get(i);
                let uid = users[i];
                match groups.get_mut(&h) {
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
                            h,
                            Bucket {
                                min_url_row: i as u32,
                                min_title_row: i as u32,
                                count: 1,
                                users: users_set,
                            },
                        );
                    }
                }
                (groups, phrase_by_hash)
            },
        )
        .reduce(
            || (AHashMap::new(), AHashMap::new()),
            |(mut a_groups, mut a_phrases), (b_groups, b_phrases)| {
                for (h, phrase) in b_phrases {
                    a_phrases.entry(h).or_insert(phrase);
                }
                for (h, b) in b_groups {
                    a_groups
                        .entry(h)
                        .and_modify(|a| merge_bucket(a, &b, urls, titles))
                        .or_insert(b);
                }
                (a_groups, a_phrases)
            },
        );

    finish_q23(groups, phrase_by_hash, urls, titles, limit, offset)
}

fn merge_bucket(
    a: &mut Bucket,
    b: &Bucket,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
) {
    if urls.get(b.min_url_row as usize) < urls.get(a.min_url_row as usize) {
        a.min_url_row = b.min_url_row;
    }
    if titles.get(b.min_title_row as usize) < titles.get(a.min_title_row as usize) {
        a.min_title_row = b.min_title_row;
    }
    a.count += b.count;
    a.users.extend(b.users.iter().copied());
}

fn finish_q23(
    groups: AHashMap<u64, Bucket>,
    phrase_by_hash: AHashMap<u64, String>,
    urls: &crate::storage::Utf8Column,
    titles: &crate::storage::Utf8Column,
    limit: usize,
    offset: usize,
) -> Vec<Vec<String>> {
    let scored = groups.into_iter().map(|(h, b)| {
        (
            b.count,
            vec![
                phrase_by_hash[&h].clone(),
                urls.get(b.min_url_row as usize).to_string(),
                titles.get(b.min_title_row as usize).to_string(),
                b.count.to_string(),
                b.users.len().to_string(),
            ],
        )
    });
    top_counts(scored, limit, offset)
}

fn is_col(e: &sqlparser::ast::Expr, name: &str) -> bool {
    crate::sql::expr_column_name(e).as_deref() == Some(name)
}
