//! Block-reader scaffold for V2 column layouts with V1 fallback.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::column::{open_column_payload, ColumnType, ENC_LZ4, ENC_RAW, FORMAT_V1, MAGIC};
use crate::Result;

const BLOCKS_SCHEMA_VERSION: u8 = 1;
const BLOCKS_SIDECAR_SUFFIX: &str = ".blocks.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockEncoding {
    Raw,
    Lz4,
}

impl BlockEncoding {
    fn decode(self, bytes: &[u8]) -> Result<Vec<u8>> {
        match self {
            BlockEncoding::Raw => Ok(bytes.to_vec()),
            BlockEncoding::Lz4 => lz4_flex::decompress_size_prepended(bytes)
                .map_err(|e| crate::Error::msg(format!("lz4 block decompress: {e}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnBlockMeta {
    pub block_id: usize,
    pub row_start: usize,
    pub row_count: usize,
    pub compressed_offset: u64,
    pub compressed_len: u64,
    pub decompressed_len: u64,
    pub encoding: BlockEncoding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnBlocksSidecar {
    pub schema_version: u8,
    pub column_type: ColumnType,
    pub row_count: usize,
    pub block_rows: usize,
    pub blocks: Vec<ColumnBlockMeta>,
}

impl ColumnBlocksSidecar {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != BLOCKS_SCHEMA_VERSION {
            return Err(crate::Error::msg(format!(
                "unsupported block sidecar schema version {}",
                self.schema_version
            )));
        }
        if self.block_rows == 0 {
            return Err(crate::Error::msg("block_rows must be > 0"));
        }
        if self.blocks.is_empty() && self.row_count != 0 {
            return Err(crate::Error::msg("sidecar blocks missing for non-empty column"));
        }

        let mut expected_id = 0usize;
        let mut expected_row = 0usize;
        for block in &self.blocks {
            if block.block_id != expected_id {
                return Err(crate::Error::msg("non-contiguous block_id in sidecar"));
            }
            if block.row_start != expected_row {
                return Err(crate::Error::msg("non-contiguous row ranges in sidecar"));
            }
            if block.row_count == 0 {
                return Err(crate::Error::msg("block row_count must be > 0"));
            }
            if block.compressed_len == 0 && block.decompressed_len > 0 {
                return Err(crate::Error::msg("compressed_len is zero for non-empty block"));
            }
            expected_id += 1;
            expected_row += block.row_count;
        }
        if expected_row != self.row_count {
            return Err(crate::Error::msg("sidecar row_count does not match block rows"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ColumnBlock {
    pub meta: ColumnBlockMeta,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ColumnBlockReader {
    path: PathBuf,
    column_type: ColumnType,
    sidecar: ColumnBlocksSidecar,
    fallback_payload: Option<Vec<u8>>,
}

impl ColumnBlockReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(sidecar) = read_blocks_sidecar(&path)? {
            sidecar.validate()?;
            let reader = Self {
                path,
                column_type: sidecar.column_type,
                sidecar,
                fallback_payload: None,
            };
            return Ok(reader);
        }

        // V1 fallback: expose the whole decompressed payload as a single block.
        let (column_type, payload) = open_column_payload(&path)?;
        let row_count = if payload.len() < 8 {
            0usize
        } else {
            u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize
        };
        let compressed_len = std::fs::metadata(&path)
            .map(|m| m.len().saturating_sub(7))
            .unwrap_or(0);
        let sidecar = ColumnBlocksSidecar {
            schema_version: BLOCKS_SCHEMA_VERSION,
            column_type,
            row_count,
            block_rows: row_count.max(1),
            blocks: vec![ColumnBlockMeta {
                block_id: 0,
                row_start: 0,
                row_count,
                compressed_offset: 0,
                compressed_len,
                decompressed_len: payload.len() as u64,
                encoding: BlockEncoding::Raw,
            }],
        };
        Ok(Self {
            path,
            column_type,
            sidecar,
            fallback_payload: Some(payload),
        })
    }

    pub fn column_type(&self) -> ColumnType {
        self.column_type
    }

    pub fn row_count(&self) -> usize {
        self.sidecar.row_count
    }

    pub fn block_rows(&self) -> usize {
        self.sidecar.block_rows
    }

    pub fn blocks(&self) -> &[ColumnBlockMeta] {
        &self.sidecar.blocks
    }

    pub fn iter_blocks(&self) -> impl Iterator<Item = &ColumnBlockMeta> {
        self.sidecar.blocks.iter()
    }

    pub fn read_block(&self, block_id: usize) -> Result<ColumnBlock> {
        let meta = self
            .sidecar
            .blocks
            .get(block_id)
            .ok_or_else(|| crate::Error::msg(format!("block_id out of range: {block_id}")))?
            .clone();

        if let Some(payload) = &self.fallback_payload {
            if block_id != 0 {
                return Err(crate::Error::msg("v1 fallback exposes only block 0"));
            }
            return Ok(ColumnBlock {
                meta,
                bytes: payload.clone(),
            });
        }

        let data = read_file_range(&self.path, meta.compressed_offset, meta.compressed_len)?;
        let bytes = meta.encoding.decode(&data)?;
        if meta.decompressed_len != 0 && bytes.len() as u64 != meta.decompressed_len {
            return Err(crate::Error::msg(format!(
                "block {} decompressed size mismatch: expected {}, got {}",
                meta.block_id,
                meta.decompressed_len,
                bytes.len()
            )));
        }
        Ok(ColumnBlock { meta, bytes })
    }
}

fn read_blocks_sidecar(col_path: &Path) -> Result<Option<ColumnBlocksSidecar>> {
    let sidecar_path = sidecar_path_for(col_path);
    if !sidecar_path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(sidecar_path)?;
    let sidecar: ColumnBlocksSidecar = serde_json::from_str(&json)?;
    Ok(Some(sidecar))
}

fn sidecar_path_for(col_path: &Path) -> PathBuf {
    let mut p = col_path.as_os_str().to_os_string();
    p.push(BLOCKS_SIDECAR_SUFFIX);
    PathBuf::from(p)
}

fn read_file_range(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut out = vec![0u8; len as usize];
    file.read_exact(&mut out)?;
    Ok(out)
}

pub(crate) fn write_blocks_sidecar(path: &Path, sidecar: &ColumnBlocksSidecar) -> Result<()> {
    if sidecar.schema_version != BLOCKS_SCHEMA_VERSION {
        return Err(crate::Error::msg(format!(
            "unsupported block sidecar schema version {}",
            sidecar.schema_version
        )));
    }
    sidecar.validate()?;
    let data = serde_json::to_string_pretty(sidecar)?;
    std::fs::write(sidecar_path_for(path), data)?;
    Ok(())
}

pub(crate) fn encode_v1_single_block(path: &Path) -> Result<ColumnBlocksSidecar> {
    let mut file = std::fs::File::open(path)?;
    let mut head = [0u8; 7];
    use std::io::Read;
    file.read_exact(&mut head)?;
    if &head[0..4] != MAGIC {
        return Err(crate::Error::msg("invalid column file magic"));
    }
    if head[4] != FORMAT_V1 {
        return Err(crate::Error::msg("unsupported column format"));
    }
    let col_type = match head[5] {
        0 => ColumnType::Int64,
        1 => ColumnType::Int32,
        2 => ColumnType::Int16,
        3 => ColumnType::Utf8,
        4 => ColumnType::Date,
        5 => ColumnType::Timestamp,
        n => return Err(crate::Error::msg(format!("unknown column type tag {n}"))),
    };
    let encoding = match head[6] {
        ENC_RAW => BlockEncoding::Raw,
        ENC_LZ4 => BlockEncoding::Lz4,
        n => return Err(crate::Error::msg(format!("unknown column encoding tag {n}"))),
    };
    let compressed_len = std::fs::metadata(path)?.len().saturating_sub(7);
    let (_, payload) = open_column_payload(path)?;
    let row_count = if payload.len() < 8 {
        0usize
    } else {
        u64::from_le_bytes(payload[0..8].try_into().unwrap()) as usize
    };
    Ok(ColumnBlocksSidecar {
        schema_version: BLOCKS_SCHEMA_VERSION,
        column_type: col_type,
        row_count,
        block_rows: row_count.max(1),
        blocks: vec![ColumnBlockMeta {
            block_id: 0,
            row_start: 0,
            row_count,
            compressed_offset: 7,
            compressed_len,
            decompressed_len: payload.len() as u64,
            encoding,
        }],
    })
}
