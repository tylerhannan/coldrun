use crate::sql::{ParsedQuery, SelectProjection};

/// If the query has ORDER BY + LIMIT and many rows, keep only the top (limit + offset) rows.
pub fn truncate_to_top_k(parsed: &ParsedQuery, columns: &[String], rows: &mut Vec<Vec<String>>) {
    let Some(limit) = parsed.limit else {
        return;
    };
    let offset = parsed.offset.unwrap_or(0) as usize;
    let need = limit as usize + offset;
    if rows.len() <= need.saturating_mul(4) {
        return;
    }
    let order_col = match resolve_first_order_column(parsed, columns, &parsed.select_items) {
        Some(i) => i,
        None => return,
    };
    let desc = parsed.order_by.first().map(|(_, d)| *d).unwrap_or(false);

    let nth = need.min(rows.len()).saturating_sub(1);
    rows.select_nth_unstable_by(nth, |a, b| {
        let cmp = compare_cell(&a[order_col], &b[order_col]);
        if desc {
            cmp.reverse()
        } else {
            cmp
        }
    });
    rows.truncate(need);
}

fn resolve_first_order_column(
    parsed: &ParsedQuery,
    columns: &[String],
    projections: &[SelectProjection],
) -> Option<usize> {
    let (expr, _) = parsed.order_by.first()?;
    if let sqlparser::ast::Expr::Identifier(ident) = expr {
        if let Some(i) = columns.iter().position(|c| c == &ident.value) {
            return Some(i);
        }
        for (i, p) in projections.iter().enumerate() {
            if p.alias.as_deref() == Some(&ident.value) {
                return Some(i);
            }
        }
    }
    if let sqlparser::ast::Expr::Function(f) = expr {
        if f.name.to_string().to_uppercase() == "COUNT" {
            if let Some(i) = columns.iter().position(|c| c == "count()") {
                return Some(i);
            }
            for (i, p) in projections.iter().enumerate() {
                if matches!(&p.kind, crate::sql::SelectItemKind::CountAll | crate::sql::SelectItemKind::Count(_)) {
                    return Some(i);
                }
            }
        }
    }
    if let Some(name) = crate::sql::expr_column_name(expr) {
        return columns.iter().position(|c| c == &name);
    }
    None
}

fn compare_cell(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}
