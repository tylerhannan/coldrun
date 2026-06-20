//! Scan one on-disk column at a time without inserting into the warm-serve cache.

use std::path::Path;

use super::column::open_column_payload;
use super::utf8_col::read_utf8_offsets;
use crate::Result;

#[inline]
pub(crate) fn utf8_row_str<'a>(body: &'a [u8], offsets: &[u64], row: usize) -> &'a str {
    let pos = offsets[row] as usize;
    let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
    let start = pos + 4;
    unsafe { std::str::from_utf8_unchecked(&body[start..start + len]) }
}

fn build_sequential_offsets(body: &[u8], row_count: usize) -> Result<Vec<u64>> {
    let mut offsets = Vec::with_capacity(row_count);
    let mut pos = 0usize;
    for _ in 0..row_count {
        if pos + 4 > body.len() {
            return Err(crate::Error::msg("utf8 payload truncated"));
        }
        offsets.push(pos as u64);
        let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4 + len;
    }
    Ok(offsets)
}

/// Decompressed UTF-8 column body + sidecar offsets. Drop after each scan pass.
pub struct Utf8ColumnScan {
    body: Vec<u8>,
    offsets: Vec<u64>,
}

impl Utf8ColumnScan {
    pub fn open(path: &Path) -> Result<Self> {
        let (_, payload) = open_column_payload(path)?;
        if payload.len() < 8 {
            return Err(crate::Error::msg("column payload truncated"));
        }
        let row_count = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
        let body = payload[8..].to_vec();
        let offsets = if let Some(offsets) = read_utf8_offsets(path) {
            offsets
        } else {
            build_sequential_offsets(&body, row_count)?
        };
        Ok(Self {
            body,
            offsets,
        })
    }

    #[inline]
    pub fn str_at(&self, row: usize) -> &str {
        utf8_row_str(&self.body, &self.offsets, row)
    }
}
pub struct Int64ColumnScan {
    values: Vec<i64>,
}

impl Int64ColumnScan {
    pub fn open(path: &Path) -> Result<Self> {
        let (_, payload) = open_column_payload(path)?;
        if payload.len() < 8 {
            return Err(crate::Error::msg("column payload truncated"));
        }
        let count = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
        let body = &payload[8..];
        let byte_len = count * size_of::<i64>();
        if byte_len > body.len() {
            return Err(crate::Error::msg("column payload truncated"));
        }
        let mut values = Vec::with_capacity(count);
        unsafe {
            let ptr = body.as_ptr() as *const i64;
            values.extend_from_slice(std::slice::from_raw_parts(ptr, count));
        }
        Ok(Self { values })
    }

    #[inline]
    pub fn at(&self, row: usize) -> i64 {
        self.values[row]
    }

    pub fn as_slice(&self) -> &[i64] {
        &self.values
    }
}
