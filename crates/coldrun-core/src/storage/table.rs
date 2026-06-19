use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::column::{ColumnData, ColumnType};
use super::zones::ZoneIndex;
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
    /// Set for synthetic demo loads: one group per row on high-card keys (Q19/Q35/Q36 fast paths).
    #[serde(default)]
    pub demo_near_unique: bool,
}

#[derive(Debug)]
pub struct Table {
    pub path: PathBuf,
    pub meta: TableMeta,
    columns: HashMap<String, ColumnData>,
    zones: Option<ZoneIndex>,
}

impl Table {
    pub fn create(path: impl AsRef<Path>, name: &str, columns: Vec<ColumnMeta>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(path.join("columns"))?;
        let meta = TableMeta {
            name: name.to_string(),
            row_count: 0,
            columns,
            demo_near_unique: false,
        };
        let table = Self {
            path,
            meta,
            columns: HashMap::new(),
            zones: None,
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
        let zones = ZoneIndex::load(&path);
        Ok(Self {
            path,
            meta,
            columns,
            zones,
        })
    }

    pub fn zones(&self) -> Option<&ZoneIndex> {
        self.zones.as_ref()
    }

    pub fn demo_near_unique(&self) -> bool {
        self.meta.demo_near_unique
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

    pub fn is_column_loaded(&self, name: &str) -> bool {
        self.columns.contains_key(name)
    }

    /// Load column files not yet in memory (two-phase scan projection). Parallel when multiple.
    pub fn load_columns(&mut self, names: &[&str]) -> Result<()> {
        let to_load: Vec<&str> = names
            .iter()
            .copied()
            .filter(|&name| !self.columns.contains_key(name))
            .collect();
        if to_load.is_empty() {
            return Ok(());
        }

        let col_dir = self.path.join("columns");
        let meta_cols = self.meta.columns.clone();

        let loaded: Vec<(String, ColumnData)> = if to_load.len() == 1 {
            let name = to_load[0];
            if !meta_cols.iter().any(|c| c.name == name) {
                Vec::new()
            } else {
                vec![(name.to_string(), Self::read_column_file(&col_dir, &meta_cols, name)?)]
            }
        } else {
            use rayon::prelude::*;
            to_load
                .par_iter()
                .copied()
                .filter(|&name| meta_cols.iter().any(|c| c.name == name))
                .map(|name| {
                    Ok((
                        name.to_string(),
                        Self::read_column_file(&col_dir, &meta_cols, name)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        };

        for (name, data) in loaded {
            self.columns.insert(name, data);
        }
        Ok(())
    }

    fn read_column_file(
        col_dir: &std::path::Path,
        meta_cols: &[ColumnMeta],
        name: &str,
    ) -> Result<ColumnData> {
        if !meta_cols.iter().any(|c| c.name == name) {
            return Err(crate::Error::msg(format!("unknown column '{name}'")));
        }
        let col_path = col_dir.join(format!("{name}.col"));
        if !col_path.exists() {
            return Err(crate::Error::msg(format!("column file missing: {name}")));
        }
        ColumnData::read_file(&col_path)
    }

    pub fn unload_columns(&self) -> Vec<String> {
        self.meta
            .columns
            .iter()
            .filter(|c| !self.columns.contains_key(&c.name))
            .map(|c| c.name.clone())
            .collect()
    }

    /// Drop decoded columns not needed for the current query (warm-serve memory cap).
    pub fn retain_columns(&mut self, keep: &std::collections::HashSet<String>) {
        self.columns
            .retain(|name, _| keep.contains(name));
    }

    /// Drop decoded columns by name (streaming kernels release memory before next scan).
    pub fn drop_columns(&mut self, names: &[&str]) {
        for name in names {
            self.columns.remove(*name);
        }
    }

    /// Project row values at `row_indices` without loading full columns into memory.
    pub fn project_rows(&self, row_indices: &[usize]) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let names: Vec<String> = self.meta.columns.iter().map(|c| c.name.clone()).collect();
        let ncols = names.len();
        let nrows = row_indices.len();
        let col_dir = self.path.join("columns");

        use rayon::prelude::*;
        let cells_by_col: Result<Vec<Vec<String>>> = self
            .meta
            .columns
            .par_iter()
            .map(|col_meta| {
                if let Ok(loaded) = self.column(&col_meta.name) {
                    Ok(row_indices
                        .iter()
                        .map(|&r| ColumnData::cell_to_string(loaded, r))
                        .collect())
                } else {
                    let path = col_dir.join(format!("{}.col", col_meta.name));
                    ColumnData::read_cells_at(&path, row_indices)
                }
            })
            .collect();

        let cells_by_col = cells_by_col?;
        let mut rows: Vec<Vec<String>> = (0..nrows).map(|_| Vec::with_capacity(ncols)).collect();
        for col_cells in cells_by_col {
            for (out, cell) in rows.iter_mut().zip(col_cells) {
                out.push(cell);
            }
        }
        Ok((names, rows))
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
        if let Some(index) = ZoneIndex::build(self) {
            index.write(&self.path)?;
            self.zones = Some(index);
        }
        self.save_meta()?;
        Ok(())
    }

    pub(crate) fn save_meta(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.meta)?;
        std::fs::write(self.path.join(META), data)?;
        Ok(())
    }

    pub fn set_row_count(&mut self, count: u64) {
        self.meta.row_count = count;
    }

    /// Build zone index from on-disk columns after a streaming load.
    pub fn build_zones_from_disk(&mut self) -> Result<()> {
        const ZONE_COLS: &[&str] = &["CounterID", "EventDate", "AdvEngineID", "EventTime"];
        let names: Vec<&str> = ZONE_COLS
            .iter()
            .copied()
            .filter(|name| self.column_type(name).is_some())
            .collect();
        if names.is_empty() {
            return Ok(());
        }
        self.load_columns(&names)?;
        if let Some(index) = ZoneIndex::build(self) {
            index.write(&self.path)?;
            self.zones = Some(index);
        }
        for name in names {
            self.columns.remove(name);
        }
        Ok(())
    }
}
