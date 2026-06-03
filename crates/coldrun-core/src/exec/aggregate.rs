use ahash::AHashSet;

use sqlparser::ast::Expr;

use crate::expr::{eval_i64, eval_string, format_date_days};
use crate::sql::{expr_column_name, SelectItemKind};
use crate::storage::{ColumnData, Table};
use crate::Result;

/// Per-group or global aggregate accumulator.
#[derive(Debug, Default)]
pub struct AggState {
    pub count_all: u64,
    pub sum: Option<i128>,
    pub avg_sum: Option<i128>,
    pub avg_count: u64,
    pub count_distinct: Option<AHashSet<String>>,
    pub count_distinct_i64: Option<AHashSet<i64>>,
    pub min_i64: Option<i64>,
    pub max_i64: Option<i64>,
    pub min_str: Option<String>,
    pub max_str: Option<String>,
}

impl AggState {
    pub fn update(&mut self, table: &Table, item: &SelectItemKind, row: usize) -> Result<()> {
        match item {
            SelectItemKind::CountAll | SelectItemKind::Count(_) => {
                self.count_all += 1;
            }
            SelectItemKind::Sum(expr) => {
                let v = if let Some(v) = eval_i64_column(table, expr, row) {
                    v as i128
                } else {
                    eval_i64(table, expr, row)? as i128
                };
                *self.sum.get_or_insert(0) += v;
            }
            SelectItemKind::Avg(expr) => {
                let v = if let Some(v) = eval_i64_column(table, expr, row) {
                    v as i128
                } else {
                    eval_i64(table, expr, row)? as i128
                };
                *self.avg_sum.get_or_insert(0) += v;
                self.avg_count += 1;
            }
            SelectItemKind::CountDistinct(expr) => {
                if let Some(name) = expr_column_name(expr) {
                    if let Ok(col) = table.column(&name) {
                        if let Some(v) = i64_at_column(col, row) {
                            self.count_distinct_i64.get_or_insert_default().insert(v);
                            return Ok(());
                        }
                    }
                }
                let key = if let Some(name) = expr_column_name(expr) {
                    col_key_from_column(table.column(&name)?, row)?
                } else {
                    eval_i64(table, expr, row)?.to_string()
                };
                self.count_distinct.get_or_insert_default().insert(key);
            }
            SelectItemKind::Min(expr) => {
                if let Ok(s) = eval_string(table, expr, row) {
                    match &mut self.min_str {
                        Some(cur) => {
                            if s < *cur {
                                *cur = s;
                            }
                        }
                        None => self.min_str = Some(s),
                    }
                } else {
                    let v = eval_i64(table, expr, row)?;
                    match &mut self.min_i64 {
                        Some(cur) => {
                            if v < *cur {
                                *cur = v;
                            }
                        }
                        None => self.min_i64 = Some(v),
                    }
                }
            }
            SelectItemKind::Max(expr) => {
                if let Ok(s) = eval_string(table, expr, row) {
                    match &mut self.max_str {
                        Some(cur) => {
                            if s > *cur {
                                *cur = s;
                            }
                        }
                        None => self.max_str = Some(s),
                    }
                } else {
                    let v = eval_i64(table, expr, row)?;
                    match &mut self.max_i64 {
                        Some(cur) => {
                            if v > *cur {
                                *cur = v;
                            }
                        }
                        None => self.max_i64 = Some(v),
                    }
                }
            }
            SelectItemKind::Column(_) | SelectItemKind::Other(_) => {}
        }
        Ok(())
    }

    pub fn finish(
        &self,
        item: &SelectItemKind,
        key_value: Option<&str>,
        table: Option<&Table>,
    ) -> Result<(String, String)> {
        match item {
            SelectItemKind::Column(_) => {
                Ok(("col".into(), key_value.unwrap_or_default().to_string()))
            }
            SelectItemKind::Other(_) => {
                Ok(("col".into(), key_value.unwrap_or_default().to_string()))
            }
            SelectItemKind::CountAll | SelectItemKind::Count(_) => {
                Ok(("count()".into(), self.count_all.to_string()))
            }
            SelectItemKind::Sum(_) => {
                Ok(("sum".into(), self.sum.unwrap_or(0).to_string()))
            }
            SelectItemKind::Avg(_) => {
                let sum = self.avg_sum.unwrap_or(0);
                let n = self.avg_count.max(1);
                let avg = sum as f64 / n as f64;
                Ok(("avg".into(), format!("{avg}")))
            }
            SelectItemKind::CountDistinct(_) => {
                let n = self
                    .count_distinct_i64
                    .as_ref()
                    .map(|s| s.len())
                    .or_else(|| self.count_distinct.as_ref().map(|s| s.len()))
                    .unwrap_or(0);
                Ok(("count(distinct)".into(), n.to_string()))
            }
            SelectItemKind::Min(expr) => {
                if let Some(s) = &self.min_str {
                    return Ok(("min".into(), s.clone()));
                }
                let v = self.min_i64.unwrap_or(0);
                if let (Some(table), Some(name)) = (table, expr_column_name(expr)) {
                    if matches!(table.column(&name), Ok(ColumnData::Date(_))) {
                        return Ok(("min".into(), format_date_days(v as i32)));
                    }
                }
                Ok(("min".into(), v.to_string()))
            }
            SelectItemKind::Max(expr) => {
                if let Some(s) = &self.max_str {
                    return Ok(("max".into(), s.clone()));
                }
                let v = self.max_i64.unwrap_or(0);
                if let (Some(table), Some(name)) = (table, expr_column_name(expr)) {
                    if matches!(table.column(&name), Ok(ColumnData::Date(_))) {
                        return Ok(("max".into(), format_date_days(v as i32)));
                    }
                }
                Ok(("max".into(), v.to_string()))
            }
        }
    }

    pub fn passes_having(&self, expr: &Expr) -> Result<bool> {
        // COUNT(*) > N
        if let Expr::BinaryOp {
            left,
            op: sqlparser::ast::BinaryOperator::Gt,
            right,
        } = expr
        {
            if let Expr::Function(f) = &**left {
                if f.name.to_string().to_uppercase() == "COUNT" {
                    if let Expr::Value(sqlparser::ast::Value::Number(n, _)) = &**right {
                        if let Ok(threshold) = n.parse::<u64>() {
                            return Ok(self.count_all > threshold);
                        }
                    }
                }
            }
        }
        Ok(true)
    }
}

fn eval_i64_column(table: &Table, expr: &Expr, row: usize) -> Option<i64> {
    let name = expr_column_name(expr)?;
    let col = table.column(&name).ok()?;
    i64_at_column(col, row)
}

fn i64_at_column(col: &ColumnData, row: usize) -> Option<i64> {
    Some(match col {
        ColumnData::Int64(v) => v.get(row).copied()?,
        ColumnData::Int32(v) => i64::from(*v.get(row)?),
        ColumnData::Int16(v) => i64::from(*v.get(row)?),
        ColumnData::Date(v) => i64::from(*v.get(row)?),
        ColumnData::Timestamp(v) => *v.get(row)?,
        ColumnData::Utf8(_) => return None,
    })
}

fn col_key_from_column(col: &crate::storage::ColumnData, i: usize) -> Result<String> {
    use crate::storage::ColumnData;
    Ok(match col {
        ColumnData::Int64(v) => v[i].to_string(),
        ColumnData::Int32(v) => v[i].to_string(),
        ColumnData::Int16(v) => v[i].to_string(),
        ColumnData::Date(v) => format_date_days(v[i]),
        ColumnData::Timestamp(v) => crate::expr::format_timestamp_micros(v[i]),
        ColumnData::Utf8(v) => v[i].to_string(),
    })
}

pub fn eval_global_select(
    table: &Table,
    item: &SelectItemKind,
    mask: &[bool],
) -> Result<(String, String)> {
    let mut state = AggState::default();
    for i in 0..mask.len() {
        if mask[i] {
            state.update(table, item, i)?;
        }
    }
    state.finish(item, None, Some(table))
}
