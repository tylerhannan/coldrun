use std::collections::HashSet;

use sqlparser::ast::{Expr, Function, FunctionArg, FunctionArguments};

use super::{expr_column_name, parse_query, ParsedQuery, SelectItemKind};
use crate::Result;

/// Column names referenced by a query (for pruning I/O). Returns `None` if all columns are needed (`SELECT *`).
pub fn referenced_columns_for_sql(sql: &str) -> Result<Option<HashSet<String>>> {
    let parsed = parse_query(sql)?;
    Ok(referenced_columns(&parsed))
}

/// Q24: load URL + EventTime first; project remaining columns after top-K sort.
pub fn q24_narrow_load(parsed: &ParsedQuery) -> bool {
    if !parsed.select_all || parsed.order_by.len() != 1 || parsed.limit.is_none() {
        return false;
    }
    let (order_expr, _) = &parsed.order_by[0];
    if expr_column_name(order_expr).as_deref() != Some("EventTime") {
        return false;
    }
    parsed
        .where_expr
        .as_ref()
        .is_some_and(where_is_url_like)
}

pub fn referenced_columns(parsed: &ParsedQuery) -> Option<HashSet<String>> {
    if parsed.select_all {
        if q24_narrow_load(parsed) {
            let mut cols = HashSet::new();
            cols.insert("URL".to_string());
            cols.insert("EventTime".to_string());
            return Some(cols);
        }
        return None;
    }
    let mut cols = HashSet::new();
    for proj in &parsed.select_items {
        collect_select_kind(&proj.kind, &mut cols);
    }
    if let Some(w) = &parsed.where_expr {
        collect_expr(w, &mut cols);
    }
    for g in &parsed.group_by {
        collect_expr(g, &mut cols);
    }
    if let Some(h) = &parsed.having {
        collect_expr(h, &mut cols);
    }
    for (e, _) in &parsed.order_by {
        collect_expr(e, &mut cols);
    }
    if cols.is_empty() {
        return None;
    }
    Some(cols)
}

fn collect_select_kind(kind: &SelectItemKind, cols: &mut HashSet<String>) {
    match kind {
        SelectItemKind::Sum(e)
        | SelectItemKind::Avg(e)
        | SelectItemKind::Count(e)
        | SelectItemKind::CountDistinct(e)
        | SelectItemKind::Min(e)
        | SelectItemKind::Max(e)
        | SelectItemKind::Column(e)
        | SelectItemKind::Other(e) => collect_expr(e, cols),
        SelectItemKind::CountAll => {}
    }
}

fn collect_expr(expr: &Expr, cols: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(ident) => {
            cols.insert(ident.value.clone());
        }
        Expr::CompoundIdentifier(parts) => {
            if let Some(id) = parts.last() {
                cols.insert(id.value.clone());
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_expr(left, cols);
            collect_expr(right, cols);
        }
        Expr::UnaryOp { expr: inner, .. } => collect_expr(inner, cols),
        Expr::Nested(inner) => collect_expr(inner, cols),
        Expr::Cast { expr: inner, .. } => collect_expr(inner, cols),
        Expr::Like { expr: inner, .. } | Expr::ILike { expr: inner, .. } => collect_expr(inner, cols),
        Expr::InList { expr: inner, list, .. } => {
            collect_expr(inner, cols);
            for e in list {
                collect_expr(e, cols);
            }
        }
        Expr::Function(f) => collect_function(f, cols),
        Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(op) = operand {
                collect_expr(op, cols);
            }
            for e in conditions {
                collect_expr(e, cols);
            }
            for e in results {
                collect_expr(e, cols);
            }
            if let Some(e) = else_result {
                collect_expr(e, cols);
            }
        }
        Expr::Extract { expr: inner, .. } => collect_expr(inner, cols),
        Expr::IsNull(inner) => collect_expr(inner, cols),
        Expr::Value(_) | Expr::TypedString { .. } => {}
        _ => {}
    }
}

fn where_is_url_like(expr: &Expr) -> bool {
    match expr {
        Expr::Like {
            expr: inner,
            pattern: _,
            negated: false,
            ..
        } => expr_column_name(inner).as_deref() == Some("URL"),
        Expr::Nested(e) => where_is_url_like(e),
        _ => false,
    }
}

fn collect_function(f: &Function, cols: &mut HashSet<String>) {
    if let FunctionArguments::List(list) = &f.args {
        for arg in &list.args {
            if let FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(e)) = arg {
                collect_expr(e, cols);
            }
        }
    }
}
