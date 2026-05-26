mod columns;

pub use columns::{referenced_columns, referenced_columns_for_sql};

use sqlparser::ast::{
    DuplicateTreatment, Expr, Function, FunctionArg, FunctionArguments,
    GroupByExpr, OrderBy, Select, SelectItem, SetExpr, Statement, TableFactor, TableWithJoins,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use crate::Result;

#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub from_table: String,
    pub select_items: Vec<SelectProjection>,
    pub select_all: bool,
    pub where_expr: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<(Expr, bool)>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SelectProjection {
    pub kind: SelectItemKind,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SelectItemKind {
    CountAll,
    Sum(Expr),
    Avg(Expr),
    Count(Expr),
    CountDistinct(Expr),
    Min(Expr),
    Max(Expr),
    Column(Expr),
    Other(Expr),
}

pub fn parse_query(sql: &str) -> Result<ParsedQuery> {
    let dialect = PostgreSqlDialect {};
    let statements = Parser::parse_sql(&dialect, sql)?;
    let stmt = statements
        .into_iter()
        .next()
        .ok_or_else(|| crate::Error::msg("empty query"))?;
    let Statement::Query(query) = stmt else {
        return Err(crate::Error::msg("only SELECT supported"));
    };
    let SetExpr::Select(select) = &*query.body else {
        return Err(crate::Error::msg("only simple SELECT supported"));
    };
    parse_select(select, query.order_by.as_ref(), &query.limit, query.offset.as_ref())
}

fn parse_select(
    select: &Select,
    order_by: Option<&OrderBy>,
    limit_clause: &Option<Expr>,
    offset_clause: Option<&sqlparser::ast::Offset>,
) -> Result<ParsedQuery> {
    let from_table = match select.from.first() {
        Some(TableWithJoins {
            relation: TableFactor::Table { name, .. },
            ..
        }) => name.to_string(),
        _ => return Err(crate::Error::msg("FROM table required")),
    };

    let mut select_all = false;
    let mut items = Vec::new();
    for item in &select.projection {
        if matches!(item, SelectItem::Wildcard(_)) {
            select_all = true;
            continue;
        }
        items.push(parse_projection(item)?);
    }

    let where_expr = select.selection.clone();
    let group_by = match &select.group_by {
        GroupByExpr::Expressions(exprs, _) => exprs.clone(),
        GroupByExpr::All(_) => Vec::new(),
    };

    let order_by = order_by
        .map(|o| {
            o.exprs
                .iter()
                .map(|e| (e.expr.clone(), !e.asc.unwrap_or(true)))
                .collect()
        })
        .unwrap_or_default();

    let limit = limit_clause.as_ref().and_then(|e| match e {
        Expr::Value(sqlparser::ast::Value::Number(n, _)) => n.parse().ok(),
        _ => None,
    });

    let offset = offset_clause.and_then(|o| match &o.value {
        Expr::Value(sqlparser::ast::Value::Number(n, _)) => n.parse().ok(),
        _ => None,
    });

    Ok(ParsedQuery {
        from_table,
        select_items: items,
        select_all,
        where_expr,
        group_by,
        having: select.having.clone(),
        order_by,
        limit,
        offset,
    })
}

fn parse_projection(item: &SelectItem) -> Result<SelectProjection> {
    let (expr, alias) = match item {
        SelectItem::UnnamedExpr(e) => (e.clone(), None),
        SelectItem::ExprWithAlias { expr, alias, .. } => (expr.clone(), Some(alias.value.clone())),
        _ => return Err(crate::Error::msg("unsupported select item")),
    };
    let kind = classify_expr(&expr)?;
    Ok(SelectProjection { kind, alias })
}

fn classify_expr(expr: &Expr) -> Result<SelectItemKind> {
    match expr {
        Expr::Function(f) => {
            let name = f.name.to_string().to_uppercase();
            if matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX") {
                parse_func(f)
            } else {
                Ok(SelectItemKind::Other(expr.clone()))
            }
        }
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) => Ok(SelectItemKind::Column(expr.clone())),
        Expr::Value(_)
        | Expr::BinaryOp { .. }
        | Expr::Case { .. }
        | Expr::Extract { .. }
        | Expr::Cast { .. }
        | Expr::Nested(_) => Ok(SelectItemKind::Other(expr.clone())),
        _ => Ok(SelectItemKind::Other(expr.clone())),
    }
}

fn parse_func(f: &Function) -> Result<SelectItemKind> {
    let name = f.name.to_string().to_uppercase();
    let (distinct, has_wildcard, arg, args_empty) = match &f.args {
        FunctionArguments::None => (false, false, None, true),
        FunctionArguments::List(list) => {
            let distinct = matches!(
                list.duplicate_treatment,
                Some(DuplicateTreatment::Distinct)
            );
            let has_wildcard = list.args.iter().any(|a| {
                matches!(
                    a,
                    FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Wildcard)
                )
            });
            let arg = list.args.first().and_then(|a| match a {
                FunctionArg::Unnamed(sqlparser::ast::FunctionArgExpr::Expr(e)) => Some(e.clone()),
                _ => None,
            });
            (distinct, has_wildcard, arg, list.args.is_empty())
        }
        _ => return Err(crate::Error::msg("unsupported function args")),
    };

    match (name.as_str(), distinct, has_wildcard, arg) {
        ("COUNT", false, true, _) => Ok(SelectItemKind::CountAll),
        ("COUNT", false, false, None) if args_empty => Ok(SelectItemKind::CountAll),
        ("COUNT", false, false, Some(e)) => Ok(SelectItemKind::Count(e)),
        ("COUNT", true, _, Some(e)) => Ok(SelectItemKind::CountDistinct(e)),
        ("SUM", false, _, Some(e)) => Ok(SelectItemKind::Sum(e)),
        ("AVG", false, _, Some(e)) => Ok(SelectItemKind::Avg(e)),
        ("MIN", false, _, Some(e)) => Ok(SelectItemKind::Min(e)),
        ("MAX", false, _, Some(e)) => Ok(SelectItemKind::Max(e)),
        _ => Err(crate::Error::msg(format!("unsupported function {name}"))),
    }
}

pub fn expr_column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.clone()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|i| i.value.clone()),
        _ => None,
    }
}

pub fn projection_label(proj: &SelectProjection) -> String {
    if let Some(alias) = &proj.alias {
        return alias.clone();
    }
    match &proj.kind {
        SelectItemKind::CountAll | SelectItemKind::Count(_) => "count()".into(),
        SelectItemKind::Sum(e) => format!("sum({})", expr_column_name(e).unwrap_or_default()),
        SelectItemKind::Avg(e) => format!("avg({})", expr_column_name(e).unwrap_or_default()),
        SelectItemKind::CountDistinct(e) => {
            format!("count(distinct {})", expr_column_name(e).unwrap_or_default())
        }
        SelectItemKind::Min(e) => format!("min({})", expr_column_name(e).unwrap_or_default()),
        SelectItemKind::Max(e) => format!("max({})", expr_column_name(e).unwrap_or_default()),
        SelectItemKind::Column(e) => expr_column_name(e).unwrap_or_else(|| "col".into()),
        SelectItemKind::Other(_) => "col".into(),
    }
}
