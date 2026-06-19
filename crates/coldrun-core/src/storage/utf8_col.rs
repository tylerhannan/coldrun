//! Contiguous UTF-8 column storage — scan without per-row `String` allocations.

use std::io::Read;
use std::ops::Index;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, LargeStringArray, StringArray};

use crate::Result;

const IDX_VERSION_U32: u8 = 1;
const IDX_VERSION_U64: u8 = 2;

#[derive(Debug, Clone)]
enum Utf8Storage {
    Building {
        body: Vec<u8>,
        offsets: Vec<u64>,
    },
    Frozen {
        body: Arc<[u8]>,
        offsets: Arc<[u64]>,
    },
}

#[derive(Debug, Clone)]
pub struct Utf8Column {
    storage: Utf8Storage,
}

impl Utf8Column {
    pub fn new() -> Self {
        Self {
            storage: Utf8Storage::Building {
                body: Vec::new(),
                offsets: Vec::new(),
            },
        }
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            Utf8Storage::Building { offsets, .. } => offsets.len(),
            Utf8Storage::Frozen { offsets, .. } => offsets.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn body_offsets(&self) -> (&[u8], &[u64]) {
        match &self.storage {
            Utf8Storage::Building { body, offsets } => (body, offsets),
            Utf8Storage::Frozen { body, offsets } => (body, offsets),
        }
    }

    fn ensure_building(&mut self) {
        if matches!(self.storage, Utf8Storage::Frozen { .. }) {
            let Utf8Storage::Frozen { body, offsets } =
                std::mem::replace(&mut self.storage, Utf8Storage::building_default())
            else {
                unreachable!();
            };
            self.storage = Utf8Storage::Building {
                body: body.to_vec(),
                offsets: offsets.to_vec(),
            };
        }
    }

    #[inline]
    pub fn get(&self, row: usize) -> &str {
        let (body, offsets) = self.body_offsets();
        let pos = offsets[row] as usize;
        let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
        let start = pos + 4;
        let end = start + len;
        unsafe { std::str::from_utf8_unchecked(&body[start..end]) }
    }

    pub fn push_str(&mut self, s: &str) {
        let Utf8Storage::Building { body, offsets } = &mut self.storage else {
            self.ensure_building();
            return self.push_str(s);
        };
        offsets.push(body.len() as u64);
        let bytes = s.as_bytes();
        body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(bytes);
    }

    pub fn append_string_array(&mut self, array: &StringArray) {
        let Utf8Storage::Building { body, offsets } = &mut self.storage else {
            self.ensure_building();
            return self.append_string_array(array);
        };
        offsets.reserve(array.len());
        for i in 0..array.len() {
            offsets.push(body.len() as u64);
            let bytes = if array.is_null(i) {
                &[][..]
            } else {
                array.value(i).as_bytes()
            };
            body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            body.extend_from_slice(bytes);
        }
    }

    pub fn append_large_string_array(&mut self, array: &LargeStringArray) {
        let Utf8Storage::Building { body, offsets } = &mut self.storage else {
            self.ensure_building();
            return self.append_large_string_array(array);
        };
        offsets.reserve(array.len());
        for i in 0..array.len() {
            offsets.push(body.len() as u64);
            let bytes = if array.is_null(i) {
                &[][..]
            } else {
                array.value(i).as_bytes()
            };
            body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            body.extend_from_slice(bytes);
        }
    }

    pub fn extend_from(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        self.ensure_building();
        let Utf8Storage::Building {
            body,
            offsets,
        } = &mut self.storage
        else {
            unreachable!();
        };
        let (other_body, other_offsets) = other.body_offsets();
        let base = body.len() as u64;
        body.extend_from_slice(other_body);
        offsets.reserve(other_offsets.len());
        for &off in other_offsets {
            offsets.push(base + off);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    pub fn from_body_with_sidecar(body: &[u8], col_path: &Path) -> Result<Self> {
        if let Some(offsets) = read_utf8_offsets(col_path) {
            return Ok(Self {
                storage: Utf8Storage::Frozen {
                    body: Arc::from(body),
                    offsets: Arc::from(offsets.into_boxed_slice()),
                },
            });
        }
        Ok(Self::from_sequential_body(body)?)
    }

    pub fn from_sequential_body(body: &[u8]) -> Result<Self> {
        let mut offsets = Vec::new();
        let mut pos = 0usize;
        while pos + 4 <= body.len() {
            offsets.push(pos as u64);
            let len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4 + len;
            if pos > body.len() {
                return Err(crate::Error::msg("utf8 payload truncated"));
            }
        }
        Ok(Self {
            storage: Utf8Storage::Frozen {
                body: Arc::from(body),
                offsets: Arc::from(offsets.into_boxed_slice()),
            },
        })
    }
}

impl Utf8Storage {
    fn building_default() -> Self {
        Self::Building {
            body: Vec::new(),
            offsets: Vec::new(),
        }
    }
}

impl Default for Utf8Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Index<usize> for Utf8Column {
    type Output = str;

    #[inline]
    fn index(&self, row: usize) -> &Self::Output {
        self.get(row)
    }
}

fn utf8_idx_path(col_path: &Path) -> std::path::PathBuf {
    let mut p = col_path.as_os_str().to_os_string();
    p.push(".idx");
    std::path::PathBuf::from(p)
}

pub(crate) fn read_utf8_offsets(col_path: &Path) -> Option<Vec<u64>> {
    let path = utf8_idx_path(col_path);
    let mut f = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 13];
    f.read_exact(&mut header).ok()?;
    if &header[..4] != b"CRUI" {
        return None;
    }
    let count = u64::from_le_bytes(header[5..13].try_into().ok()?) as usize;
    match header[4] {
        IDX_VERSION_U32 => {
            let mut bytes = vec![0u8; count * 4];
            f.read_exact(&mut bytes).ok()?;
            Some(
                bytes
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as u64)
                    .collect(),
            )
        }
        IDX_VERSION_U64 => {
            let mut bytes = vec![0u8; count * 8];
            f.read_exact(&mut bytes).ok()?;
            Some(
                bytes
                    .chunks_exact(8)
                    .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                    .collect(),
            )
        }
        _ => None,
    }
}

pub(crate) fn write_utf8_idx_sidecar(col_path: &Path, offsets: &[u64]) -> Result<()> {
    let path = utf8_idx_path(col_path);
    let mut out = Vec::with_capacity(4 + 1 + 8 + offsets.len() * 8);
    out.extend_from_slice(b"CRUI");
    out.push(IDX_VERSION_U64);
    out.extend_from_slice(&(offsets.len() as u64).to_le_bytes());
    for &off in offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    std::fs::write(path, out)?;
    Ok(())
}

pub(crate) fn utf8_str_at(
    body: &[u8],
    offsets: Option<&[u64]>,
    row: usize,
    count: usize,
) -> Result<String> {
    if let Some(offsets) = offsets {
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
        return String::from_utf8(body[pos..pos + len].to_vec())
            .map_err(|e| crate::Error::msg(format!("invalid utf8 in column: {e}")));
    }
    read_utf8_at(body, row, count)
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
