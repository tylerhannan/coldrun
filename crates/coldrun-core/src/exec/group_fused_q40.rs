//! Q40: dashboard filter + CASE referer src + URL dst + COUNT (no per-row interpreter).

use ahash::AHashMap;
use sqlparser::ast::Expr;

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_heap::top_counts;
use super::group_fused::build_mask;
use super::mask_util::for_each_selected;
use super::utf8_arena::Utf8Intern;
use super::QueryResult;

pub fn try_fused_q40(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !is_q40_shape(parsed) {
        return Ok(None);
    }
    let ColumnData::Int16(trafic) = table.column("TraficSourceID")? else {
        return Ok(None);
    };
    let ColumnData::Int16(se) = table.column("SearchEngineID")? else {
        return Ok(None);
    };
    let ColumnData::Int16(adv) = table.column("AdvEngineID")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(referer) = table.column("Referer")? else {
        return Ok(None);
    };
    let ColumnData::Utf8(url) = table.column("URL")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(Some(QueryResult {
            columns: parsed.select_items.iter().map(projection_label).collect(),
            rows: vec![],
        }));
    };

    let mut src_intern = Utf8Intern::with_capacity(256);
    let mut dst_intern = Utf8Intern::with_capacity(256);
    let empty_src = src_intern.intern("");
    let mut counts: AHashMap<(i16, i16, i16, u32, u32), u64> = AHashMap::with_capacity(512);

    for_each_selected(&mask, row_count, |i| {
        let t = trafic[i];
        let s = se[i];
        let a = adv[i];
        let si = if s == 0 && a == 0 {
            src_intern.intern(referer[i].as_str())
        } else {
            empty_src
        };
        let di = dst_intern.intern(url[i].as_str());
        *counts.entry((t, s, a, si, di)).or_insert(0) += 1;
    });

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let scored = counts.into_iter().map(|((t, s, a, si, di), c)| {
        (
            c,
            vec![
                t.to_string(),
                s.to_string(),
                a.to_string(),
                src_intern.get(si).to_string(),
                dst_intern.get(di).to_string(),
                c.to_string(),
            ],
        )
    });
    let rows = top_counts(scored, limit, offset);
    Ok(Some(QueryResult { columns, rows }))
}

fn is_q40_shape(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 5 {
        return false;
    }
    let mut has_case = false;
    let mut has_url = false;
    for e in &parsed.group_by {
        match e {
            Expr::Case { .. } => has_case = true,
            Expr::Identifier(id) if id.value == "URL" => has_url = true,
            Expr::Identifier(id) => {
                if !matches!(
                    id.value.as_str(),
                    "TraficSourceID" | "SearchEngineID" | "AdvEngineID"
                ) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    if !has_case || !has_url {
        return false;
    }
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll | SelectItemKind::Count(_) | SelectItemKind::Column(_)
        )
    })
}
