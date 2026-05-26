//! Zero-cost typed column slice views for fused kernels.

use crate::storage::ColumnData;

#[derive(Copy, Clone)]
pub enum IntCols<'a> {
    I16(&'a [i16]),
    I32(&'a [i32]),
    I64(&'a [i64]),
}

pub fn as_int_cols(col: &ColumnData) -> Option<IntCols<'_>> {
    if let Some(v) = col.as_i16_slice() {
        return Some(IntCols::I16(v));
    }
    if let Some(v) = col.as_i32_slice() {
        return Some(IntCols::I32(v));
    }
    if let Some(v) = col.as_i64_slice() {
        return Some(IntCols::I64(v));
    }
    None
}

#[inline]
pub fn int_at(cols: IntCols<'_>, row: usize) -> i64 {
    match cols {
        IntCols::I16(v) => i64::from(v[row]),
        IntCols::I32(v) => i64::from(v[row]),
        IntCols::I64(v) => v[row],
    }
}

#[inline]
pub fn pack_pair(c1: IntCols<'_>, c2: IntCols<'_>, row: usize) -> u128 {
    let a = int_at(c1, row) as u64;
    let b = int_at(c2, row) as u64;
    ((a as u128) << 64) | (b as u128)
}
