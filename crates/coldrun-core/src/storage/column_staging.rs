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
    name: String,
    col_type: ColumnType,
    row_count: u64,
    pod_path: Option<PathBuf>,
    utf8_body_path: Option<PathBuf>,
    utf8_off_path: Option<PathBuf>,
}

impl StreamingColumnWriter {
    pub fn new(col_dir: &Path, name: &str, ty: ColumnType) -> Result<Self> {
        fs::create_dir_all(col_dir)?;
        let writer = match ty {
            ColumnType::Utf8 => {
                let body_path = col_dir.join(format!("{name}.staging.body"));
                let off_path = col_dir.join(format!("{name}.staging.off"));
                let _ = fs::remove_file(&body_path);
                let _ = fs::remove_file(&off_path);
                File::create(&body_path)?;
                File::create(&off_path)?;
                Self {
                    name: name.to_string(),
                    col_type: ty,
                    row_count: 0,
                    pod_path: None,
                    utf8_body_path: Some(body_path),
                    utf8_off_path: Some(off_path),
                }
            }
            _ => {
                let pod_path = col_dir.join(format!("{name}.staging"));
                let _ = fs::remove_file(&pod_path);
                File::create(&pod_path)?;
                Self {
                    name: name.to_string(),
                    col_type: ty,
                    row_count: 0,
                    pod_path: Some(pod_path),
                    utf8_body_path: None,
                    utf8_off_path: None,
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
                let off_path = self
                    .utf8_off_path
                    .as_ref()
                    .ok_or_else(|| crate::Error::msg("utf8 offset staging path missing"))?;
                let mut body = std::fs::OpenOptions::new().append(true).open(body_path)?;
                let mut off = std::fs::OpenOptions::new().append(true).open(off_path)?;
                let mut body_len = body.metadata()?.len();
                for s in chunk.iter() {
                    off.write_all(&body_len.to_le_bytes())?;
                    let bytes = s.as_bytes();
                    body.write_all(&(bytes.len() as u32).to_le_bytes())?;
                    body.write_all(bytes)?;
                    body_len = body_len.checked_add(4 + bytes.len() as u64).ok_or_else(|| {
                        crate::Error::msg(format!(
                            "utf8 column '{}' body exceeded addressable size at {} bytes",
                            self.name, body_len
                        ))
                    })?;
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
                let off_path = self
                    .utf8_off_path
                    .as_ref()
                    .ok_or_else(|| crate::Error::msg("utf8 offset staging path missing"))?;
                finalize_utf8(col_path, count, body_path, off_path, &self.name)?;
                let _ = fs::remove_file(body_path);
                let _ = fs::remove_file(off_path);
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

fn read_staged_offsets(path: &Path, name: &str) -> Result<Vec<u64>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len % 8 != 0 {
        return Err(crate::Error::msg(format!(
            "utf8 column '{name}' offset staging file size {len} is not a multiple of 8"
        )));
    }
    let _count = (len / 8) as usize;
    let mut bytes = vec![0u8; len as usize];
    f.read_exact(&mut bytes)?;
    Ok(bytes
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

fn finalize_utf8(
    col_path: &Path,
    count: u64,
    body_path: &Path,
    off_path: &Path,
    name: &str,
) -> Result<()> {
    let offsets = read_staged_offsets(off_path, name)?;
    if offsets.len() as u64 != count {
        return Err(crate::Error::msg(format!(
            "utf8 column '{name}' row count mismatch: {count} rows, {} offsets",
            offsets.len()
        )));
    }
    let body_len = fs::metadata(body_path)?.len();
    let raw_len = 8 + body_len;
    if raw_len > STREAM_RAW_THRESHOLD {
        write_utf8_idx_sidecar(col_path, &offsets)?;
        write_streaming_raw(col_path, ColumnType::Utf8, count, body_path, body_len)?;
        return Ok(());
    }
    let mut body = Vec::with_capacity(body_len as usize);
    File::open(body_path)?.read_to_end(&mut body)?;
    let mut raw = Vec::with_capacity(8 + body.len());
    raw.extend_from_slice(&count.to_le_bytes());
    raw.extend_from_slice(&body);
    write_col_payload(col_path, ColumnType::Utf8, &raw, Some(&offsets))
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
