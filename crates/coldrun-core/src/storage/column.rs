use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::pod::PodStorage;
use crate::Result;

const MAGIC: &[u8; 4] = b"CRUN";
const IDX_MAGIC: &[u8; 4] = b"CRUI";
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
                let mut offsets = Vec::with_capacity(v.len());
                let mut pos = 0usize;
                for s in v {
                    offsets.push(pos as u32);
                    let bytes = s.as_bytes();
                    let len = bytes.len() as u32;
                    raw.extend_from_slice(&len.to_le_bytes());
                    raw.extend_from_slice(bytes);
                    pos += 4 + bytes.len();
                }
                write_utf8_idx_sidecar(path, &offsets)?;
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
        let (col_type, payload) = open_column_payload(path)?;
        decode_column_payload_typed(&payload, col_type)
    }

    /// Read formatted cell strings at row indices without materializing the full column.
    pub fn read_cells_at(path: &Path, rows: &[usize]) -> Result<Vec<String>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let (col_type, payload) = open_column_payload(path)?;
        let utf8_offsets = if col_type == ColumnType::Utf8 {
            read_utf8_offsets(path)
        } else {
            None
        };
        decode_cells_at(&payload, col_type, rows, utf8_offsets.as_deref())
    }

    pub fn cell_to_string(col: &ColumnData, row: usize) -> String {
        match col {
            ColumnData::Int64(v) => v[row].to_string(),
            ColumnData::Int32(v) => v[row].to_string(),
            ColumnData::Int16(v) => v[row].to_string(),
            ColumnData::Date(v) => v[row].to_string(),
            ColumnData::Timestamp(v) => v[row].to_string(),
            ColumnData::Utf8(v) => v[row].clone(),
        }
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

fn open_column_payload(path: &Path) -> Result<(ColumnType, Vec<u8>)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    let data = if len > 64 * 1024 {
        unsafe { memmap2::Mmap::map(&file)? }.to_vec()
    } else {
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        buf
    };
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
    Ok((col_type, payload))
}

fn decode_column_payload_typed(payload: &[u8], col_type: ColumnType) -> Result<ColumnData> {
    if payload.len() < 8 {
        return Err(crate::Error::msg("column payload truncated"));
    }
    let count = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
    let body = &payload[8..];
    Ok(match col_type {
        ColumnType::Int64 => ColumnData::Int64(read_pod_slice(body, count)?),
        ColumnType::Int32 => ColumnData::Int32(read_pod_slice(body, count)?),
        ColumnType::Int16 => ColumnData::Int16(read_pod_slice(body, count)?),
        ColumnType::Date => ColumnData::Date(read_pod_slice(body, count)?),
        ColumnType::Timestamp => ColumnData::Timestamp(read_pod_slice(body, count)?),
        ColumnType::Utf8 => ColumnData::Utf8(read_utf8_vec(body, count)?),
    })
}

fn decode_cells_at(
    payload: &[u8],
    col_type: ColumnType,
    rows: &[usize],
    utf8_offsets: Option<&[u32]>,
) -> Result<Vec<String>> {
    if payload.len() < 8 {
        return Err(crate::Error::msg("column payload truncated"));
    }
    let count = u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize;
    let body = &payload[8..];
    rows.iter()
        .map(|&row| {
            if row >= count {
                return Err(crate::Error::msg("row index out of range"));
            }
            format_cell_at(body, col_type, row, count, utf8_offsets)
        })
        .collect()
}

fn format_cell_at(
    body: &[u8],
    col_type: ColumnType,
    row: usize,
    count: usize,
    utf8_offsets: Option<&[u32]>,
) -> Result<String> {
    Ok(match col_type {
        ColumnType::Int64 => read_pod_at::<i64>(body, row)?.to_string(),
        ColumnType::Int32 => read_pod_at::<i32>(body, row)?.to_string(),
        ColumnType::Int16 => read_pod_at::<i16>(body, row)?.to_string(),
        ColumnType::Date => read_pod_at::<i32>(body, row)?.to_string(),
        ColumnType::Timestamp => read_pod_at::<i64>(body, row)?.to_string(),
        ColumnType::Utf8 => {
            if let Some(offsets) = utf8_offsets {
                read_utf8_at_offset(body, offsets, row)?
            } else {
                read_utf8_at(body, row, count)?
            }
        }
    })
}

fn read_pod_at<T: Copy>(body: &[u8], row: usize) -> Result<T> {
    let size = size_of::<T>();
    let off = row * size;
    if off + size > body.len() {
        return Err(crate::Error::msg("pod row out of range"));
    }
    Ok(unsafe { std::ptr::read_unaligned(body.as_ptr().add(off) as *const T) })
}

fn read_pod_slice<T: Copy>(body: &[u8], count: usize) -> Result<PodStorage<T>> {
    let byte_len = count * size_of::<T>();
    if byte_len > body.len() {
        return Err(crate::Error::msg("column payload truncated"));
    }
    let mut vec = Vec::with_capacity(count);
    unsafe {
        let ptr = body.as_ptr() as *const T;
        vec.extend_from_slice(std::slice::from_raw_parts(ptr, count));
    }
    Ok(PodStorage::from_arc(Arc::from(vec.into_boxed_slice())))
}

fn read_utf8_vec(body: &[u8], count: usize) -> Result<Vec<String>> {
    let mut strings = Vec::with_capacity(count);
    let mut pos = 0usize;
    for _ in 0..count {
        if pos + 4 > body.len() {
            return Err(crate::Error::msg("utf8 payload truncated"));
        }
        let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + len > body.len() {
            return Err(crate::Error::msg("utf8 payload truncated"));
        }
        strings.push(
            String::from_utf8(body[pos..pos + len].to_vec())
                .map_err(|e| crate::Error::msg(format!("invalid utf8 in column: {e}")))?,
        );
        pos += len;
    }
    Ok(strings)
}

fn utf8_idx_path(col_path: &Path) -> std::path::PathBuf {
    let mut p = col_path.as_os_str().to_os_string();
    p.push(".idx");
    std::path::PathBuf::from(p)
}

fn write_utf8_idx_sidecar(col_path: &Path, offsets: &[u32]) -> Result<()> {
    let path = utf8_idx_path(col_path);
    let mut out = Vec::with_capacity(4 + 1 + 8 + offsets.len() * 4);
    out.extend_from_slice(IDX_MAGIC);
    out.push(FORMAT_V1);
    out.extend_from_slice(&(offsets.len() as u64).to_le_bytes());
    for &off in offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    let mut f = File::create(path)?;
    f.write_all(&out)?;
    Ok(())
}

fn read_utf8_offsets(col_path: &Path) -> Option<Vec<u32>> {
    let path = utf8_idx_path(col_path);
    let mut f = File::open(path).ok()?;
    let mut header = [0u8; 13];
    f.read_exact(&mut header).ok()?;
    if &header[..4] != IDX_MAGIC || header[4] != FORMAT_V1 {
        return None;
    }
    let count = u64::from_le_bytes(header[5..13].try_into().ok()?) as usize;
    let mut bytes = vec![0u8; count * 4];
    f.read_exact(&mut bytes).ok()?;
    Some(
        bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect(),
    )
}

fn read_utf8_at_offset(body: &[u8], offsets: &[u32], row: usize) -> Result<String> {
    let mut pos = *offsets
        .get(row)
        .ok_or_else(|| crate::Error::msg("row index out of range"))? as usize;
    if pos + 4 > body.len() {
        return Err(crate::Error::msg("utf8 payload truncated"));
    }
    let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    if pos + len > body.len() {
        return Err(crate::Error::msg("utf8 payload truncated"));
    }
    String::from_utf8(body[pos..pos + len].to_vec())
        .map_err(|e| crate::Error::msg(format!("invalid utf8 in column: {e}")))
}

fn read_utf8_at(body: &[u8], mut row: usize, count: usize) -> Result<String> {
    let mut pos = 0usize;
    for _ in 0..count {
        if pos + 4 > body.len() {
            return Err(crate::Error::msg("utf8 payload truncated"));
        }
        let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
        if row == 0 {
            pos += 4;
            if pos + len > body.len() {
                return Err(crate::Error::msg("utf8 payload truncated"));
            }
            return String::from_utf8(body[pos..pos + len].to_vec())
                .map_err(|e| crate::Error::msg(format!("invalid utf8 in column: {e}")));
        }
        pos += 4 + len;
        row -= 1;
    }
    Err(crate::Error::msg("row index out of range"))
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
