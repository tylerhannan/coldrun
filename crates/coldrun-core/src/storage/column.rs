use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::pod::PodStorage;
use crate::Result;

const MAGIC: &[u8; 4] = b"CRUN";
const FORMAT_V1: u8 = 1;
const ENC_RAW: u8 = 0;
const ENC_LZ4: u8 = 1;

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
    Int64(PodStorage<i64>),
    Int32(PodStorage<i32>),
    Int16(PodStorage<i16>),
    Utf8(Vec<String>),
    Date(PodStorage<i32>),
    Timestamp(PodStorage<i64>),
}

impl ColumnData {
    pub fn as_i64_slice(&self) -> Option<&[i64]> {
        match self {
            ColumnData::Int64(v) | ColumnData::Timestamp(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_i32_slice(&self) -> Option<&[i32]> {
        match self {
            ColumnData::Int32(v) | ColumnData::Date(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_i16_slice(&self) -> Option<&[i16]> {
        match self {
            ColumnData::Int16(v) => Some(v),
            _ => None,
        }
    }

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
            _ => Err(crate::Error::msg("type mismatch")),
        }
    }

    pub fn push_i32(&mut self, v: i32) -> Result<()> {
        match self {
            Self::Int32(c) => c.push(v),
            Self::Date(c) => c.push(v),
            _ => Err(crate::Error::msg("type mismatch")),
        }
    }

    pub fn push_i16(&mut self, v: i16) -> Result<()> {
        match self {
            Self::Int16(c) => c.push(v),
            _ => Err(crate::Error::msg("type mismatch")),
        }
    }

    pub fn push_utf8(&mut self, v: String) -> Result<()> {
        match self {
            Self::Utf8(c) => {
                c.push(v);
                Ok(())
            }
            _ => Err(crate::Error::msg("type mismatch")),
        }
    }

    pub fn push_timestamp(&mut self, v: i64) -> Result<()> {
        match self {
            Self::Timestamp(c) => c.push(v),
            _ => Err(crate::Error::msg("type mismatch")),
        }
    }

    pub fn write_file(&self, path: &Path) -> Result<()> {
        let mut raw = Vec::new();
        let count = self.len() as u64;
        raw.extend_from_slice(&count.to_le_bytes());
        match self {
            ColumnData::Int64(v) => write_pod_vec(&mut raw, v),
            ColumnData::Int32(v) => write_pod_vec(&mut raw, v),
            ColumnData::Int16(v) => write_pod_vec(&mut raw, v),
            ColumnData::Date(v) => write_pod_vec(&mut raw, v),
            ColumnData::Timestamp(v) => write_pod_vec(&mut raw, v),
            ColumnData::Utf8(v) => {
                for s in v {
                    let bytes = s.as_bytes();
                    let len = bytes.len() as u32;
                    raw.extend_from_slice(&len.to_le_bytes());
                    raw.extend_from_slice(bytes);
                }
            }
        }
        let payload = if raw.len() > 4096 {
            lz4_flex::compress_prepend_size(&raw)
        } else {
            raw.clone()
        };
        let encoding = if payload.len() < raw.len() {
            ENC_LZ4
        } else {
            ENC_RAW
        };
        let body = if encoding == ENC_LZ4 {
            &payload
        } else {
            &raw
        };

        let mut f = File::create(path)?;
        f.write_all(MAGIC)?;
        f.write_all(&[FORMAT_V1])?;
        f.write_all(&[self.column_type() as u8])?;
        f.write_all(&[encoding])?;
        f.write_all(body)?;
        Ok(())
    }

    pub fn read_file(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len() as usize;
        if len > 64 * 1024 {
            let map = unsafe { memmap2::Mmap::map(&file)? };
            return decode_column_file(&map[..]);
        }

        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        decode_column_file(&data)
    }

    pub fn extend_from(&mut self, other: &ColumnData) -> Result<()> {
        match (self, other) {
            (ColumnData::Int64(d), ColumnData::Int64(s)) => d.extend_from_slice(s),
            (ColumnData::Int32(d), ColumnData::Int32(s)) => d.extend_from_slice(s),
            (ColumnData::Int16(d), ColumnData::Int16(s)) => d.extend_from_slice(s),
            (ColumnData::Date(d), ColumnData::Date(s)) => d.extend_from_slice(s),
            (ColumnData::Timestamp(d), ColumnData::Timestamp(s)) => d.extend_from_slice(s),
            (ColumnData::Utf8(d), ColumnData::Utf8(s)) => {
                d.extend_from_slice(s);
                Ok(())
            }
            _ => Err(crate::Error::msg("extend_from type mismatch")),
        }
    }
}

fn decode_column_file(data: &[u8]) -> Result<ColumnData> {
    if data.len() < 5 {
        return Err(crate::Error::msg("column file too short"));
    }
    if &data[..4] != MAGIC {
        return Err(crate::Error::msg("invalid column file magic"));
    }
    let first = data[4];
    let (col_type, encoding, body) = if first == FORMAT_V1 {
        if data.len() < 7 {
            return Err(crate::Error::msg("column file truncated"));
        }
        (parse_col_type(data[5])?, data[6], &data[7..])
    } else {
        (parse_col_type(first)?, ENC_RAW, &data[5..])
    };

    let payload = if encoding == ENC_LZ4 {
        lz4_flex::decompress_size_prepended(body)
            .map_err(|e| crate::Error::msg(format!("lz4 decompress: {e}")))?
    } else {
        body.to_vec()
    };

    let mut cursor = std::io::Cursor::new(payload);
    let mut count_buf = [0u8; 8];
    std::io::Read::read_exact(&mut cursor, &mut count_buf)?;
    let count = u64::from_le_bytes(count_buf) as usize;
    Ok(match col_type {
        ColumnType::Int64 => ColumnData::Int64(read_pod_arc(&mut cursor, count)?),
        ColumnType::Int32 => ColumnData::Int32(read_pod_arc(&mut cursor, count)?),
        ColumnType::Int16 => ColumnData::Int16(read_pod_arc(&mut cursor, count)?),
        ColumnType::Date => ColumnData::Date(read_pod_arc(&mut cursor, count)?),
        ColumnType::Timestamp => ColumnData::Timestamp(read_pod_arc(&mut cursor, count)?),
        ColumnType::Utf8 => {
            let mut strings = Vec::with_capacity(count);
            for _ in 0..count {
                let mut len_buf = [0u8; 4];
                std::io::Read::read_exact(&mut cursor, &mut len_buf)?;
                let len = u32::from_le_bytes(len_buf) as usize;
                let mut bytes = vec![0u8; len];
                std::io::Read::read_exact(&mut cursor, &mut bytes)?;
                strings.push(String::from_utf8(bytes).map_err(|e| {
                    crate::Error::msg(format!("invalid utf8 in column: {e}"))
                })?);
            }
            ColumnData::Utf8(strings)
        }
    })
}

fn parse_col_type(tag: u8) -> Result<ColumnType> {
    Ok(match tag {
        0 => ColumnType::Int64,
        1 => ColumnType::Int32,
        2 => ColumnType::Int16,
        3 => ColumnType::Utf8,
        4 => ColumnType::Date,
        5 => ColumnType::Timestamp,
        n => return Err(crate::Error::msg(format!("unknown column type tag {n}"))),
    })
}

fn write_pod_vec<T: Copy>(out: &mut Vec<u8>, data: &[T]) {
    let bytes =
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * size_of::<T>()) };
    out.extend_from_slice(bytes);
}

fn read_pod_arc<T: Copy>(
    cursor: &mut std::io::Cursor<Vec<u8>>,
    count: usize,
) -> Result<PodStorage<T>> {
    let byte_len = count * size_of::<T>();
    let pos = cursor.position() as usize;
    let buf = cursor.get_ref();
    if pos + byte_len > buf.len() {
        return Err(crate::Error::msg("column payload truncated"));
    }
    let mut vec = Vec::with_capacity(count);
    unsafe {
        let ptr = buf.as_ptr().add(pos) as *const T;
        vec.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    cursor.set_position((pos + byte_len) as u64);
    Ok(PodStorage::from_arc(Arc::from(vec.into_boxed_slice())))
}

pub fn empty_column(ty: ColumnType) -> ColumnData {
    match ty {
        ColumnType::Int64 => ColumnData::Int64(PodStorage::owned_with_capacity(0)),
        ColumnType::Int32 => ColumnData::Int32(PodStorage::owned_with_capacity(0)),
        ColumnType::Int16 => ColumnData::Int16(PodStorage::owned_with_capacity(0)),
        ColumnType::Utf8 => ColumnData::Utf8(Vec::new()),
        ColumnType::Date => ColumnData::Date(PodStorage::owned_with_capacity(0)),
        ColumnType::Timestamp => ColumnData::Timestamp(PodStorage::owned_with_capacity(0)),
    }
}
