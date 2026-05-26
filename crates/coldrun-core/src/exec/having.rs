//! HAVING clause shortcuts.

use sqlparser::ast::{BinaryOperator, Expr};

/// False when no group can satisfy `HAVING COUNT(*) > N` (e.g. N ≥ filtered row count).
pub fn having_can_match(having: &Expr, max_rows_in_any_group: u64) -> bool {
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Gt,
        right,
    } = having
    else {
        return true;
    };
    let Expr::Function(f) = &**left else {
        return true;
    };
    if f.name.to_string().to_uppercase() != "COUNT" {
        return true;
    }
    let Expr::Value(sqlparser::ast::Value::Number(n, _)) = &**right else {
        return true;
    };
    let Ok(threshold) = n.parse::<u64>() else {
        return true;
    };
    threshold < max_rows_in_any_group
}
