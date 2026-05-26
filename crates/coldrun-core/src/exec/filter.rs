use sqlparser::ast::Expr;

use crate::expr::eval_bool;
use crate::storage::Table;
use crate::Result;

pub fn build_filter_mask(
    table: &Table,
    where_expr: Option<&Expr>,
    row_count: usize,
) -> Result<Vec<bool>> {
    let Some(expr) = where_expr else {
        return Ok(vec![true; row_count]);
    };
    let mut mask = Vec::with_capacity(row_count);
    for i in 0..row_count {
        mask.push(eval_bool(table, expr, i)?);
    }
    Ok(mask)
}

pub fn eval_having_bool(table: &Table, expr: &Expr, row: usize, mask: &[bool]) -> Result<bool> {
    // HAVING is evaluated on grouped data; for simple COUNT(*) > N we special-case via group module.
    let _ = (table, expr, row, mask);
    Err(crate::Error::msg("HAVING eval not in filter"))
}
