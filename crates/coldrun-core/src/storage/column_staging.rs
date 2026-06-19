//! Append Parquet batches to on-disk staging files, then finalize into `.col` files.
//! Keeps peak RAM bounded to one record batch plus one column finalize buffer.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::column::{write_col_payload, ColumnData, ColumnType};
use super::utf8_col::write_utf8_idx_sidecar;
use crate::Result;

const STREAM_RAW_THRESHOLD: u64 = 256 * 1024 * 1024;

pub struct StreamingColumnWriter {
    col_type: ColumnType,
    row_count: u64,
    pod_path: Option<PathBuf>,
    utf8_body_path: Option<PathBuf>,
    utf8_offsets: Vec<u32>,
}

impl StreamingColumnWriter {
    pub fn new(col_dir: &Path, name: &str, ty: ColumnType) -> Result<Self> {
        fs::create_dir_all(col_dir)?;
        let writer = match ty {
            ColumnType::Utf8 => {
                let body_path = col_dir.join(format!("{name}.staging.body"));
                let _ = fs::remove_file(&body_path);
                File::create(&body_path)?;
                Self {
                    col_type: ty,
                    row_count: 0,
                    pod_path: None,
                    utf8_body_path: Some(body_path),
                    utf8_offsets: Vec::new(),
                }
            }
            _ => {
                let pod_path = col_dir.join(format!("{name}.staging"));
                let _ = fs::remove_file(&pod_path);
                File::create(&pod_path)?;
                Self {
                    col_type: ty,
                    row_count: 0,
                    pod_path: Some(pod_path),
                    utf8_body_path: None,
                    utf8_offsets: Vec::new(),
                }
            }
        };
        Ok(writer)
    }

    pub fn append(&mut self, chunk: &ColumnData) -> Result<()> {
        if chunk.column_type() != self.col_type {
            return Err(crate::Error::msg("staging append type mismatch"));
        }
        let n = chunk.len() as u64;
        self.row_count += n;
        match (self.col_type, chunk) {
            (ColumnType::Utf8, ColumnData::Utf8(chunk)) => {
                let body_path = self
                    .utf8_body_path
                    .as_ref()
                    .ok_or_else(|| crate::Error::msg("utf8 staging path missing"))?;
                let mut body = std::fs::OpenOptions::new().append(true).open(body_path)?;
                let base = body.metadata()?.len();
                if base > u32::MAX as u64 {
                    return Err(crate::Error::msg("utf8 staging body too large"));
                }
                let mut body_len = base as u32;
                self.utf8_offsets.reserve(chunk.len());
                for s in chunk.iter() {
                    self.utf8_offsets.push(body_len);
                    let bytes = s.as_bytes();
                    body.write_all(&(bytes.len() as u32).to_le_bytes())?;
                    body.write_all(bytes)?;
                    body_len = body_len
                        .checked_add(4 + bytes.len() as u32)
                        .ok_or_else(|| crate::Error::msg("utf8 staging body overflow"))?;
                }
            }
            (ColumnType::Int64, ColumnData::Int64(v)) => append_pod(self.pod_path.as_ref(), v)?,
            (ColumnType::Int32, ColumnData::Int32(v)) => append_pod(self.pod_path.as_ref(), v)?,
            (ColumnType::Int16, ColumnData::Int16(v)) => append_pod(self.pod_path.as_ref(), v)?,
            (ColumnType::Date, ColumnData::Date(v)) => append_pod(self.pod_path.as_ref(), v)?,
            (ColumnType::Timestamp, ColumnData::Timestamp(v)) => {
                append_pod(self.pod_path.as_ref(), v)?
            }
            _ => return Err(crate::Error::msg("staging append type mismatch")),
        }
        Ok(())
    }

    pub fn finalize(self, col_path: &Path) -> Result<u64> {
        let count = self.row_count;
        match self.col_type {
            ColumnType::Utf8 => {
                let body_path = self
                    .utf8_body_path
                    .as_ref()
                    .ok_or_else(|| crate::Error::msg("utf8 staging path missing"))?;
                finalize_utf8(col_path, count, body_path, &self.utf8_offsets)?;
                let _ = fs::remove_file(body_path);
            }
            _ => {
                let pod_path = self
                    .pod_path
                    .as_ref()
                    .ok_or_else(|| crate::Error::msg("pod staging path missing"))?;
                finalize_pod(col_path, self.col_type, count, pod_path)?;
                let _ = fs::remove_file(pod_path);
            }
        }
        Ok(count)
    }
}

fn append_pod<T: Copy>(path: Option<&PathBuf>, values: &[T]) -> Result<()> {
    let path = path.ok_or_else(|| crate::Error::msg("pod staging path missing"))?;
    if values.is_empty() {
        return Ok(());
    }
    let bytes = pod_bytes(values);
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn pod_bytes<T: Copy>(values: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            values.as_ptr() as *const u8,
            values.len() * std::mem::size_of::<T>(),
        )
    }
}

fn finalize_pod(col_path: &Path, col_type: ColumnType, count: u64, staging_path: &Path) -> Result<()> {
    let mut staging = File::open(staging_path)?;
    let mut pod_bytes = Vec::new();
    staging.read_to_end(&mut pod_bytes)?;
    let mut raw = Vec::with_capacity(8 + pod_bytes.len());
    raw.extend_from_slice(&count.to_le_bytes());
    raw.extend_from_slice(&pod_bytes);
    write_col_payload(col_path, col_type, &raw, None)
}

fn finalize_utf8(
    col_path: &Path,
    count: u64,
    body_path: &Path,
    offsets: &[u32],
) -> Result<()> {
    let body_len = fs::metadata(body_path)?.len();
    let raw_len = 8 + body_len;
    if raw_len > STREAM_RAW_THRESHOLD {
        write_utf8_idx_sidecar(col_path, offsets)?;
        write_streaming_raw(col_path, ColumnType::Utf8, count, body_path, body_len)?;
        return Ok(());
    }
    let mut body = Vec::with_capacity(body_len as usize);
    File::open(body_path)?.read_to_end(&mut body)?;
    let mut raw = Vec::with_capacity(8 + body.len());
    raw.extend_from_slice(&count.to_le_bytes());
    raw.extend_from_slice(&body);
    write_col_payload(col_path, ColumnType::Utf8, &raw, Some(offsets))
}

fn write_streaming_raw(
    col_path: &Path,
    col_type: ColumnType,
    count: u64,
    body_path: &Path,
    body_len: u64,
) -> Result<()> {
    use super::column::{ENC_RAW, FORMAT_V1, MAGIC};

    let mut out = File::create(col_path)?;
    out.write_all(MAGIC)?;
    out.write_all(&[FORMAT_V1])?;
    out.write_all(&[col_type as u8])?;
    out.write_all(&[ENC_RAW])?;
    out.write_all(&count.to_le_bytes())?;
    let mut body = File::open(body_path)?;
    std::io::copy(&mut body, &mut out)?;
    let _ = body_len;
    Ok(())
}
