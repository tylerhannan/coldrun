//! Q29: referer host + AVG(length(Referer)) + COUNT + MIN(Referer), HAVING, top 25.

use ahash::AHashMap;
use sqlparser::ast::{Expr, Function, FunctionArg, FunctionArguments};

use crate::expr::referer_host_str;
use crate::sql::{projection_label, ParsedQuery, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

use super::group_fused::{build_mask, parse_having_count_gt};
use super::mask_util::for_each_selected;
use super::utf8_arena::Utf8Intern;
use super::QueryResult;

pub fn try_fused_q29(
    table: &Table,
    parsed: &ParsedQuery,
    row_count: usize,
) -> Result<Option<QueryResult>> {
    if !is_q29_shape(parsed) {
        return Ok(None);
    }
    let Some(threshold) = parse_having_count_gt(parsed.having.as_ref()) else {
        return Ok(None);
    };
    let ColumnData::Utf8(referer) = table.column("Referer")? else {
        return Ok(None);
    };

    let Some(mask) = build_mask(table, parsed, row_count)? else {
        return Ok(Some(QueryResult {
            columns: parsed.select_items.iter().map(projection_label).collect(),
            rows: vec![],
        }));
    };

    #[derive(Default)]
    struct HostAgg {
        count: u64,
        sum_len: u128,
        min_referer: Option<String>,
    }

    let mut intern = Utf8Intern::with_capacity(4096);
    let mut groups: AHashMap<u32, HostAgg> = AHashMap::with_capacity(4096);

    for_each_selected(&mask, row_count, |i| {
        let s = referer[i].as_str();
        if s.is_empty() {
            return;
        }
        let host = referer_host_str(s);
        let hid = intern.intern(host);
        let b = groups.entry(hid).or_default();
        b.count += 1;
        b.sum_len += s.chars().count() as u128;
        match &b.min_referer {
            Some(cur) if s >= cur.as_str() => {}
            _ => b.min_referer = Some(referer[i].clone()),
        }
    });

    let limit = parsed.limit.map(|l| l as usize).unwrap_or(25);
    let offset = parsed.offset.unwrap_or(0) as usize;

    let mut scored: Vec<(f64, String, Vec<String>)> = groups
        .into_iter()
        .filter(|(_, b)| b.count > threshold)
        .map(|(hid, b)| {
            let avg = b.sum_len as f64 / b.count as f64;
            let host = intern.get(hid).to_string();
            (
                avg,
                host.clone(),
                vec![
                    host,
                    format!("{avg}"),
                    b.count.to_string(),
                    b.min_referer.unwrap_or_default(),
                ],
            )
        })
        .collect();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });

    let columns: Vec<String> = parsed.select_items.iter().map(projection_label).collect();
    let rows: Vec<Vec<String>> = scored
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, _, row)| row)
        .collect();

    Ok(Some(QueryResult { columns, rows }))
}

fn is_q29_shape(parsed: &ParsedQuery) -> bool {
    if parsed.group_by.len() != 1 {
        return false;
    }
    let resolved = super::group::resolve_group_expr(&parsed.group_by[0], &parsed.select_items);
    if !is_referer_regexp_expr(&resolved) && !is_referer_group_alias(parsed, &resolved) {
        return false;
    }
    let mut has_avg_len = false;
    let mut has_count = false;
    let mut has_min = false;
    for p in &parsed.select_items {
        match &p.kind {
            SelectItemKind::Avg(e) if is_length_referer(e) => has_avg_len = true,
            SelectItemKind::CountAll | SelectItemKind::Count(_) => has_count = true,
            SelectItemKind::Min(e) if expr_is_referer(e) => has_min = true,
            SelectItemKind::Other(e) if is_referer_regexp_expr(e) => {}
            _ => return false,
        }
    }
    has_avg_len && has_count && has_min
}

fn is_referer_group_alias(parsed: &ParsedQuery, expr: &Expr) -> bool {
    let Expr::Identifier(id) = expr else {
        return false;
    };
    parsed.select_items.iter().any(|p| {
        p.alias.as_deref() == Some(&id.value)
            && matches!(&p.kind, SelectItemKind::Other(e) if is_referer_regexp_expr(e))
    })
}

fn is_referer_regexp_expr(expr: &Expr) -> bool {
    let Expr::Function(f) = expr else {
        return false;
    };
    if f.name.to_string().to_uppercase() != "REGEXP_REPLACE" {
        return false;
    }
    let list = match &f.args {
        FunctionArguments::List(l) => &l.args,
        _ => return false,
    };
    let Some(FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(Expr::Identifier(id)))) =
        list.first()
    else {
        return false;
    };
    id.value == "Referer"
}

fn is_length_referer(expr: &Expr) -> bool {
    let Expr::Function(f) = expr else {
        return false;
    };
    if f.name.to_string().to_uppercase() != "LENGTH" {
        return false;
    }
    expr_is_referer(extract_function_arg0(f).unwrap_or(&Expr::Value(sqlparser::ast::Value::Null)))
}

fn expr_is_referer(expr: &Expr) -> bool {
    matches!(expr, Expr::Identifier(id) if id.value == "Referer")
}

fn extract_function_arg0(f: &Function) -> Option<&Expr> {
    match &f.args {
        FunctionArguments::List(l) => match l.args.first()? {
            FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(e)) => Some(e),
            _ => None,
        },
        _ => None,
    }
}
