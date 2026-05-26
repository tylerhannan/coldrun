use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::column::{ColumnData, ColumnType};
use crate::Result;

const META: &str = "meta.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub ty: ColumnType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMeta {
    pub name: String,
    pub row_count: u64,
    pub columns: Vec<ColumnMeta>,
}

#[derive(Debug)]
pub struct Table {
    pub path: PathBuf,
    pub meta: TableMeta,
    columns: HashMap<String, ColumnData>,
}

impl Table {
    pub fn create(path: impl AsRef<Path>, name: &str, columns: Vec<ColumnMeta>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(path.join("columns"))?;
        let meta = TableMeta {
            name: name.to_string(),
            row_count: 0,
            columns,
        };
        let table = Self {
            path,
            meta,
            columns: HashMap::new(),
        };
        table.save_meta()?;
        Ok(table)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_columns(path, None)
    }

    /// Load only the listed columns (plus metadata). `None` loads every column file.
    pub fn open_columns(
        path: impl AsRef<Path>,
        only: Option<&std::collections::HashSet<String>>,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let meta: TableMeta = serde_json::from_str(&std::fs::read_to_string(path.join(META))?)?;
        let mut columns = HashMap::new();
        for col in &meta.columns {
            if let Some(set) = only {
                if !set.contains(&col.name) {
                    continue;
                }
            }
            let col_path = path.join("columns").join(format!("{}.col", col.name));
            if col_path.exists() {
                columns.insert(col.name.clone(), ColumnData::read_file(&col_path)?);
            }
        }
        Ok(Self {
            path,
            meta,
            columns,
        })
    }

    pub fn row_count(&self) -> u64 {
        self.meta.row_count
    }

    pub fn column_names(&self) -> impl Iterator<Item = &str> {
        self.meta.columns.iter().map(|c| c.name.as_str())
    }

    pub fn column_type(&self, name: &str) -> Option<ColumnType> {
        self.meta
            .columns
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.ty)
    }

    pub fn column(&self, name: &str) -> Result<&ColumnData> {
        self.columns
            .get(name)
            .ok_or_else(|| crate::Error::msg(format!("column '{name}' not loaded")))
    }

    pub fn column_mut(&mut self, name: &str) -> Result<&mut ColumnData> {
        self.columns
            .get_mut(name)
            .ok_or_else(|| crate::Error::msg(format!("column '{name}' not loaded")))
    }

    pub fn ensure_column(&mut self, name: &str, ty: ColumnType) -> Result<&mut ColumnData> {
        if !self.columns.contains_key(name) {
            self.columns
                .insert(name.to_string(), super::column::empty_column(ty));
            if !self.meta.columns.iter().any(|c| c.name == name) {
                self.meta.columns.push(ColumnMeta {
                    name: name.to_string(),
                    ty,
                });
            }
        }
        Ok(self.columns.get_mut(name).unwrap())
    }

    pub fn flush(&mut self) -> Result<()> {
        for col in &self.meta.columns {
            if let Some(data) = self.columns.get(&col.name) {
                let path = self
                    .path
                    .join("columns")
                    .join(format!("{}.col", col.name));
                data.write_file(&path)?;
            }
        }
        self.save_meta()?;
        Ok(())
    }

    fn save_meta(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.meta)?;
        std::fs::write(self.path.join(META), data)?;
        Ok(())
    }

    pub fn set_row_count(&mut self, count: u64) {
        self.meta.row_count = count;
    }
}
