use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;

const MAGIC: &[u8; 4] = b"CRUN";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColumnType {
    Int64,
    Int32,
    Int16,
    Utf8,
    Date,
    Timestamp,
}

#[derive(Debug, Clone)]
pub enum ColumnData {
    Int64(Vec<i64>),
    Int32(Vec<i32>),
    Int16(Vec<i16>),
    Utf8(Vec<String>),
    Date(Vec<i32>),
    Timestamp(Vec<i64>),
}

impl ColumnData {
    pub fn len(&self) -> usize {
        match self {
            Self::Int64(v) => v.len(),
            Self::Int32(v) => v.len(),
            Self::Int16(v) => v.len(),
            Self::Utf8(v) => v.len(),
            Self::Date(v) => v.len(),
            Self::Timestamp(v) => v.len(),
        }
    }

    pub fn column_type(&self) -> ColumnType {
        match self {
            Self::Int64(_) => ColumnType::Int64,
            Self::Int32(_) => ColumnType::Int32,
            Self::Int16(_) => ColumnType::Int16,
            Self::Utf8(_) => ColumnType::Utf8,
            Self::Date(_) => ColumnType::Date,
            Self::Timestamp(_) => ColumnType::Timestamp,
        }
    }

    pub fn push_i64(&mut self, v: i64) -> Result<()> {
        match self {
            Self::Int64(c) => c.push(v),
            _ => return Err(crate::Error::msg("type mismatch")),
        }
        Ok(())
    }

    pub fn push_i32(&mut self, v: i32) -> Result<()> {
        match self {
            Self::Int32(c) => c.push(v),
            Self::Date(c) => c.push(v),
            _ => return Err(crate::Error::msg("type mismatch")),
        }
        Ok(())
    }

    pub fn push_i16(&mut self, v: i16) -> Result<()> {
        match self {
            Self::Int16(c) => c.push(v),
            _ => return Err(crate::Error::msg("type mismatch")),
        }
        Ok(())
    }

    pub fn push_utf8(&mut self, v: String) -> Result<()> {
        match self {
            Self::Utf8(c) => c.push(v),
            _ => return Err(crate::Error::msg("type mismatch")),
        }
        Ok(())
    }

    pub fn push_timestamp(&mut self, v: i64) -> Result<()> {
        match self {
            Self::Timestamp(c) => c.push(v),
            _ => return Err(crate::Error::msg("type mismatch")),
        }
        Ok(())
    }

    pub fn write_file(&self, path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        f.write_all(MAGIC)?;
        let col_type = self.column_type() as u8;
        f.write_all(&[col_type])?;
        let count = self.len() as u64;
        f.write_all(&count.to_le_bytes())?;
        match self {
            ColumnData::Int64(v) => write_pod_slice(&mut f, v),
            ColumnData::Int32(v) => write_pod_slice(&mut f, v),
            ColumnData::Int16(v) => write_pod_slice(&mut f, v),
            ColumnData::Date(v) => write_pod_slice(&mut f, v),
            ColumnData::Timestamp(v) => write_pod_slice(&mut f, v),
            ColumnData::Utf8(v) => {
                for s in v {
                    let bytes = s.as_bytes();
                    let len = bytes.len() as u32;
                    f.write_all(&len.to_le_bytes())?;
                    f.write_all(bytes)?;
                }
                Ok(())
            }
        }
    }

    pub fn read_file(path: &Path) -> Result<Self> {
        let mut f = File::open(path)?;
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(crate::Error::msg("invalid column file magic"));
        }
        let mut type_byte = [0u8; 1];
        f.read_exact(&mut type_byte)?;
        let col_type = match type_byte[0] {
            0 => ColumnType::Int64,
            1 => ColumnType::Int32,
            2 => ColumnType::Int16,
            3 => ColumnType::Utf8,
            4 => ColumnType::Date,
            5 => ColumnType::Timestamp,
            n => return Err(crate::Error::msg(format!("unknown column type tag {n}"))),
        };
        let mut count_buf = [0u8; 8];
        f.read_exact(&mut count_buf)?;
        let count = u64::from_le_bytes(count_buf) as usize;
        Ok(match col_type {
            ColumnType::Int64 => ColumnData::Int64(read_pod_slice(&mut f, count)?),
            ColumnType::Int32 => ColumnData::Int32(read_pod_slice(&mut f, count)?),
            ColumnType::Int16 => ColumnData::Int16(read_pod_slice(&mut f, count)?),
            ColumnType::Date => ColumnData::Date(read_pod_slice(&mut f, count)?),
            ColumnType::Timestamp => ColumnData::Timestamp(read_pod_slice(&mut f, count)?),
            ColumnType::Utf8 => {
                let mut strings = Vec::with_capacity(count);
                for _ in 0..count {
                    let mut len_buf = [0u8; 4];
                    f.read_exact(&mut len_buf)?;
                    let len = u32::from_le_bytes(len_buf) as usize;
                    let mut bytes = vec![0u8; len];
                    f.read_exact(&mut bytes)?;
                    strings.push(String::from_utf8(bytes).map_err(|e| {
                        crate::Error::msg(format!("invalid utf8 in column: {e}"))
                    })?);
                }
                ColumnData::Utf8(strings)
            }
        })
    }
}

fn write_pod_slice<T: Copy>(f: &mut File, data: &[T]) -> Result<()> {
    let bytes =
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * size_of::<T>()) };
    f.write_all(bytes)?;
    Ok(())
}

fn read_pod_slice<T: Copy>(f: &mut File, count: usize) -> Result<Vec<T>> {
    let byte_len = count * size_of::<T>();
    let mut bytes = vec![0u8; byte_len];
    f.read_exact(&mut bytes)?;
    let mut out = Vec::with_capacity(count);
    unsafe {
        let ptr = bytes.as_ptr() as *const T;
        out.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    Ok(out)
}

pub fn empty_column(ty: ColumnType) -> ColumnData {
    match ty {
        ColumnType::Int64 => ColumnData::Int64(Vec::new()),
        ColumnType::Int32 => ColumnData::Int32(Vec::new()),
        ColumnType::Int16 => ColumnData::Int16(Vec::new()),
        ColumnType::Utf8 => ColumnData::Utf8(Vec::new()),
        ColumnType::Date => ColumnData::Date(Vec::new()),
        ColumnType::Timestamp => ColumnData::Timestamp(Vec::new()),
    }
}
