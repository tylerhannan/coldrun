//! Q40: dashboard filter + CASE referer src + URL dst + COUNT (no per-row interpreter).

use std::hash::{Hash, Hasher};

use ahash::{AHashMap, AHasher};
use sqlparser::ast::Expr;

use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::agg_topk::StreamingTopK;
use super::group_fused::build_mask;
use super::mask_util::for_each_selected;
use super::QueryResult;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Q40Key(i16, i16, i16, u64, u64);

impl PartialOrd for Q40Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Q40Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .cmp(&other.0)
            .then(self.1.cmp(&other.1))
            .then(self.2.cmp(&other.2))
            .then(self.3.cmp(&other.3))
            .then(self.4.cmp(&other.4))
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h = AHasher::default();
    s.hash(&mut h);
    h.finish()
}

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

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let offset = parsed.offset.unwrap_or(0) as usize;
    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();

    let mut samples: AHashMap<Q40Key, (String, String)> = AHashMap::with_capacity(512);
    let mut topk = StreamingTopK::with_prune_factor(limit, offset, 32);

    for_each_selected(&mask, row_count, |i| {
        let t = trafic[i];
        let s = se[i];
        let a = adv[i];
        let (sh, src_s) = if s == 0 && a == 0 {
            let r = referer[i].as_str();
            (hash_str(r), r)
        } else {
            (0, "")
        };
        let u = url[i].as_str();
        let dh = hash_str(u);
        let key = Q40Key(t, s, a, sh, dh);
        samples.entry(key).or_insert_with(|| (src_s.to_string(), u.to_string()));
        topk.inc(key);
    });

    let rows = topk.finish(|key, c| {
        let (src, dst) = &samples[&key];
        vec![
            key.0.to_string(),
            key.1.to_string(),
            key.2.to_string(),
            src.clone(),
            dst.clone(),
            c.to_string(),
        ]
    });

    Ok(Some(QueryResult { columns, rows }))
}

fn is_q40_shape(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 5 {
        return false;
    }
    let mut has_src = false;
    let mut has_dst = false;
    for e in &parsed.group_by {
        match e {
            Expr::Case { .. } => has_src = true,
            Expr::Identifier(id) if id.value == "URL" || id.value == "Dst" => has_dst = true,
            Expr::Identifier(id) if id.value == "Src" => has_src = true,
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
    if !has_src || !has_dst {
        return false;
    }
    parsed.select_items.iter().all(|p| {
        matches!(
            p.kind,
            SelectItemKind::CountAll
                | SelectItemKind::Count(_)
                | SelectItemKind::Column(_)
                | SelectItemKind::Other(Expr::Case { .. })
        )
    })
}
