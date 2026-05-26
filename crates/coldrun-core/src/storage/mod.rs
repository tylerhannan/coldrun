mod column;
mod demo;
mod load;
mod table;
pub mod zones;

pub use column::{ColumnData, ColumnType};
pub use demo::load_demo_hits;
pub use load::load_parquet_into_table;
pub use table::Table;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sql::ParsedQuery;
use crate::{Error, Result};

const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub tables: Vec<String>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            tables: Vec::new(),
        }
    }
}

/// Embedded database directory (`.coldrun/`).
#[derive(Debug)]
pub struct Database {
    pub root: PathBuf,
    manifest: Manifest,
}

impl Database {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let manifest_path = root.join(MANIFEST);
        let manifest = if manifest_path.exists() {
            let data = std::fs::read_to_string(&manifest_path)?;
            serde_json::from_str(&data)?
        } else {
            Manifest::default()
        };
        Ok(Self { root, manifest })
    }

    pub fn table_path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.manifest.tables.iter().any(|t| t == name)
    }

    pub fn open_table(&self, name: &str) -> Result<Table> {
        if !self.has_table(name) {
            return Err(Error::msg(format!("table '{name}' does not exist")));
        }
        Table::open(self.table_path(name))
    }

    /// Open a table loading only columns referenced by the parsed query.
    pub fn open_table_for_query(&self, name: &str, parsed: &ParsedQuery) -> Result<Table> {
        if !self.has_table(name) {
            return Err(Error::msg(format!("table '{name}' does not exist")));
        }
        let cols = crate::sql::referenced_columns(parsed);
        Table::open_columns(self.table_path(name), cols.as_ref())
    }

    pub fn register_table(&mut self, name: &str) -> Result<()> {
        if !self.manifest.tables.iter().any(|t| t == name) {
            self.manifest.tables.push(name.to_string());
            self.save_manifest()?;
        }
        Ok(())
    }

    fn save_manifest(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.manifest)?;
        std::fs::write(self.root.join(MANIFEST), data)?;
        Ok(())
    }

    pub fn data_size_bytes(&self) -> Result<u64> {
        dir_size(&self.root)
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p)?;
            } else {
                total += entry.metadata()?.len();
            }
        }
    }
    Ok(total)
}
