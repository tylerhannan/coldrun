use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::column_blocks::{
    write_blocks_sidecar, BlockEncoding, ColumnBlockMeta, ColumnBlocksSidecar,
};
use super::pod::PodStorage;
use super::utf8_col::{read_utf8_offsets, utf8_str_at, write_utf8_idx_sidecar, Utf8Column};
use crate::Result;

pub(crate) const MAGIC: &[u8; 4] = b"CRUN";
pub(crate) const FORMAT_V1: u8 = 1;
pub(crate) const FORMAT_V2: u8 = 2;
pub(crate) const ENC_RAW: u8 = 0;
pub(crate) const ENC_LZ4: u8 = 1;
const DEFAULT_BLOCK_ROWS: usize = 65_536;

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
    Utf8(Utf8Column),
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

    pub fn push_utf8(&mut self, v: &str) -> Result<()> {
        match self {
            Self::Utf8(c) => {
                c.push_str(v);
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
        let (raw, utf8_offsets) = self.encode_payload()?;
        write_col_payload(path, self.column_type(), &raw, utf8_offsets.as_deref())
    }

    fn encode_payload(&self) -> Result<(Vec<u8>, Option<Vec<u64>>)> {
        let mut raw = Vec::new();
        let count = self.len() as u64;
        raw.extend_from_slice(&count.to_le_bytes());
        let utf8_offsets = match self {
            ColumnData::Int64(v) => {
                write_pod_vec(&mut raw, v);
                None
            }
            ColumnData::Int32(v) => {
                write_pod_vec(&mut raw, v);
                None
            }
            ColumnData::Int16(v) => {
                write_pod_vec(&mut raw, v);
                None
            }
            ColumnData::Date(v) => {
                write_pod_vec(&mut raw, v);
                None
            }
            ColumnData::Timestamp(v) => {
                write_pod_vec(&mut raw, v);
                None
            }
            ColumnData::Utf8(v) => {
                let mut offsets = Vec::with_capacity(v.len());
                let mut pos = 0u64;
                for s in v.iter() {
                    offsets.push(pos);
                    let bytes = s.as_bytes();
                    let len = bytes.len() as u32;
                    raw.extend_from_slice(&len.to_le_bytes());
                    raw.extend_from_slice(bytes);
                    pos += 4 + bytes.len() as u64;
                }
                Some(offsets)
            }
        };
        Ok((raw, utf8_offsets))
    }

    pub fn read_file(path: &Path) -> Result<Self> {
        let (col_type, payload) = open_column_payload(path)?;
        decode_column_payload_typed(&payload, col_type, Some(path))
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
            ColumnData::Utf8(v) => v[row].to_string(),
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
                d.extend_from(s);
                Ok(())
            }
            _ => Err(crate::Error::msg("extend_from type mismatch")),
        }
    }
}

pub(crate) fn open_column_payload(path: &Path) -> Result<(ColumnType, Vec<u8>)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    if len < 5 {
        return Err(crate::Error::msg("column file too short"));
    }
    let mut head = [0u8; 6];
    file.read_exact(&mut head)?;
    if &head[0..4] != MAGIC {
        return Err(crate::Error::msg("invalid column file magic"));
    }
    if head[4] == FORMAT_V2 {
        let col_type = parse_col_type(head[5])?;
        let reader = super::column_blocks::ColumnBlockReader::open(path)?;
        if reader.column_type() != col_type {
            return Err(crate::Error::msg("column type mismatch between v2 header and sidecar"));
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(&(reader.row_count() as u64).to_le_bytes());
        for meta in reader.iter_blocks() {
            let block = reader.read_block(meta.block_id)?;
            if block.bytes.len() < 8 {
                return Err(crate::Error::msg("column block payload truncated"));
            }
            let block_rows = u64::from_le_bytes(block.bytes[0..8].try_into().unwrap()) as usize;
            if block_rows != meta.row_count {
                return Err(crate::Error::msg("column block row_count mismatch"));
            }
            payload.extend_from_slice(&block.bytes[8..]);
        }
        return Ok((col_type, payload));
    }
    if len > 64 * 1024 {
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        return decode_column_payload_from_bytes(&mmap);
    }
    drop(file);
    let mut data = Vec::new();
    File::open(path)?.read_to_end(&mut data)?;
    decode_column_payload_from_bytes(&data)
}

fn decode_column_payload_from_bytes(data: &[u8]) -> Result<(ColumnType, Vec<u8>)> {
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

fn decode_column_payload_typed(payload: &[u8], col_type: ColumnType, col_path: Option<&Path>) -> Result<ColumnData> {
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
        ColumnType::Utf8 => {
            let col = if let Some(path) = col_path {
                Utf8Column::from_body_with_sidecar(body, path)?
            } else {
                Utf8Column::from_sequential_body(body)?
            };
            if col.len() != count {
                return Err(crate::Error::msg("utf8 row count mismatch"));
            }
            ColumnData::Utf8(col)
        }
    })
}

fn decode_cells_at(
    payload: &[u8],
    col_type: ColumnType,
    rows: &[usize],
    utf8_offsets: Option<&[u64]>,
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
    utf8_offsets: Option<&[u64]>,
) -> Result<String> {
    Ok(match col_type {
        ColumnType::Int64 => read_pod_at::<i64>(body, row)?.to_string(),
        ColumnType::Int32 => read_pod_at::<i32>(body, row)?.to_string(),
        ColumnType::Int16 => read_pod_at::<i16>(body, row)?.to_string(),
        ColumnType::Date => read_pod_at::<i32>(body, row)?.to_string(),
        ColumnType::Timestamp => read_pod_at::<i64>(body, row)?.to_string(),
        ColumnType::Utf8 => utf8_str_at(body, utf8_offsets, row, count)?,
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

pub(crate) fn write_col_payload(
    path: &Path,
    col_type: ColumnType,
    raw: &[u8],
    utf8_offsets: Option<&[u64]>,
) -> Result<()> {
    if let Some(offsets) = utf8_offsets {
        write_utf8_idx_sidecar(path, offsets)?;
    }
    if raw.len() < 8 {
        return Err(crate::Error::msg("column payload truncated"));
    }
    let row_count = u64::from_le_bytes(raw[0..8].try_into().unwrap()) as usize;
    let body = &raw[8..];
    write_v2_payload(path, col_type, row_count, body, utf8_offsets, DEFAULT_BLOCK_ROWS)
}

pub(crate) fn write_utf8_col_payload_from_staging(
    path: &Path,
    row_count: usize,
    body_path: &Path,
    body_len: u64,
    offsets: &[u64],
) -> Result<()> {
    write_utf8_idx_sidecar(path, offsets)?;
    let mut out = File::create(path)?;
    out.write_all(MAGIC)?;
    out.write_all(&[FORMAT_V2])?;
    out.write_all(&[ColumnType::Utf8 as u8])?;

    let mut sidecar = ColumnBlocksSidecar {
        schema_version: 1,
        column_type: ColumnType::Utf8,
        row_count,
        block_rows: DEFAULT_BLOCK_ROWS.max(1),
        blocks: Vec::new(),
    };

    use std::io::{Read, Seek, SeekFrom};
    let mut body_file = File::open(body_path)?;
    let mut row_start = 0usize;
    let mut file_off = 6u64;
    while row_start < row_count {
        let rows = (row_count - row_start).min(DEFAULT_BLOCK_ROWS);
        let row_end = row_start + rows;
        let start = offsets[row_start] as usize;
        let end = if row_end < row_count {
            offsets[row_end] as usize
        } else {
            body_len as usize
        };
        let mut block = Vec::with_capacity(8 + end.saturating_sub(start));
        block.extend_from_slice(&(rows as u64).to_le_bytes());
        body_file.seek(SeekFrom::Start(start as u64))?;
        let mut body = vec![0u8; end.saturating_sub(start)];
        body_file.read_exact(&mut body)?;
        block.extend_from_slice(&body);
        let (encoded, encoding) = encode_block(&block);
        out.write_all(&encoded)?;
        sidecar.blocks.push(ColumnBlockMeta {
            block_id: sidecar.blocks.len(),
            row_start,
            row_count: rows,
            compressed_offset: file_off,
            compressed_len: encoded.len() as u64,
            decompressed_len: block.len() as u64,
            encoding,
        });
        file_off += encoded.len() as u64;
        row_start = row_end;
    }
    write_blocks_sidecar(path, &sidecar)
}

fn write_v2_payload(
    path: &Path,
    col_type: ColumnType,
    row_count: usize,
    body: &[u8],
    utf8_offsets: Option<&[u64]>,
    block_rows: usize,
) -> Result<()> {
    let mut out = File::create(path)?;
    out.write_all(MAGIC)?;
    out.write_all(&[FORMAT_V2])?;
    out.write_all(&[col_type as u8])?;

    let mut sidecar = ColumnBlocksSidecar {
        schema_version: 1,
        column_type: col_type,
        row_count,
        block_rows: block_rows.max(1),
        blocks: Vec::new(),
    };

    let mut row_start = 0usize;
    let mut file_off = 6u64;
    while row_start < row_count {
        let rows = (row_count - row_start).min(block_rows.max(1));
        let row_end = row_start + rows;
        let block_body = block_body_slice(col_type, row_start, row_end, row_count, body, utf8_offsets)?;
        let mut block = Vec::with_capacity(8 + block_body.len());
        block.extend_from_slice(&(rows as u64).to_le_bytes());
        block.extend_from_slice(block_body);
        let (encoded, encoding) = encode_block(&block);
        out.write_all(&encoded)?;
        sidecar.blocks.push(ColumnBlockMeta {
            block_id: sidecar.blocks.len(),
            row_start,
            row_count: rows,
            compressed_offset: file_off,
            compressed_len: encoded.len() as u64,
            decompressed_len: block.len() as u64,
            encoding,
        });
        file_off += encoded.len() as u64;
        row_start = row_end;
    }
    write_blocks_sidecar(path, &sidecar)
}

fn encode_block(block: &[u8]) -> (Vec<u8>, BlockEncoding) {
    if block.len() <= 4096 {
        return (block.to_vec(), BlockEncoding::Raw);
    }
    let compressed = lz4_flex::compress_prepend_size(block);
    if compressed.len() < block.len() {
        (compressed, BlockEncoding::Lz4)
    } else {
        (block.to_vec(), BlockEncoding::Raw)
    }
}

fn block_body_slice<'a>(
    col_type: ColumnType,
    row_start: usize,
    row_end: usize,
    row_count: usize,
    body: &'a [u8],
    utf8_offsets: Option<&[u64]>,
) -> Result<&'a [u8]> {
    match col_type {
        ColumnType::Int16 => slice_fixed(body, row_start, row_end, size_of::<i16>()),
        ColumnType::Int32 | ColumnType::Date => slice_fixed(body, row_start, row_end, size_of::<i32>()),
        ColumnType::Int64 | ColumnType::Timestamp => {
            slice_fixed(body, row_start, row_end, size_of::<i64>())
        }
        ColumnType::Utf8 => {
            let offsets = utf8_offsets.ok_or_else(|| crate::Error::msg("utf8 offsets missing"))?;
            if offsets.len() != row_count {
                return Err(crate::Error::msg("utf8 offsets row count mismatch"));
            }
            let start = offsets[row_start] as usize;
            let end = if row_end < row_count {
                offsets[row_end] as usize
            } else {
                body.len()
            };
            if end < start || end > body.len() {
                return Err(crate::Error::msg("utf8 block body bounds invalid"));
            }
            Ok(&body[start..end])
        }
    }
}

fn slice_fixed(body: &[u8], row_start: usize, row_end: usize, width: usize) -> Result<&[u8]> {
    let start = row_start
        .checked_mul(width)
        .ok_or_else(|| crate::Error::msg("fixed-width block start overflow"))?;
    let end = row_end
        .checked_mul(width)
        .ok_or_else(|| crate::Error::msg("fixed-width block end overflow"))?;
    if end < start || end > body.len() {
        return Err(crate::Error::msg("fixed-width block bounds invalid"));
    }
    Ok(&body[start..end])
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
        ColumnType::Utf8 => ColumnData::Utf8(Utf8Column::new()),
        ColumnType::Date => ColumnData::Date(PodStorage::owned_with_capacity(0)),
        ColumnType::Timestamp => ColumnData::Timestamp(PodStorage::owned_with_capacity(0)),
    }
}
