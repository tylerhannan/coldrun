use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::column::ColumnData;
use super::table::Table;
use crate::Result;

pub const ZONE_ROWS: usize = 8192;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Zone {
    pub min_counter: i32,
    pub max_counter: i32,
    pub min_date: i32,
    pub max_date: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneIndex {
    pub zones: Vec<Zone>,
}

impl ZoneIndex {
    pub fn build(table: &Table) -> Option<Self> {
        let counter = table.column("CounterID").ok()?;
        let date = table.column("EventDate").ok()?;
        let (ColumnData::Int32(counters), ColumnData::Date(dates)) = (counter, date) else {
            return None;
        };
        let n = counters.len().min(dates.len());
        if n == 0 {
            return None;
        }
        let mut zones = Vec::new();
        let mut z = 0;
        while z < n {
            let end = (z + ZONE_ROWS).min(n);
            let slice_c = &counters[z..end];
            let slice_d = &dates[z..end];
            zones.push(Zone {
                min_counter: *slice_c.iter().min().unwrap_or(&0),
                max_counter: *slice_c.iter().max().unwrap_or(&0),
                min_date: *slice_d.iter().min().unwrap_or(&0),
                max_date: *slice_d.iter().max().unwrap_or(&0),
            });
            z = end;
        }
        Some(Self { zones })
    }

    pub fn write(&self, table_path: &Path) -> Result<()> {
        let dir = table_path.join("pk_index");
        std::fs::create_dir_all(&dir)?;
        let data = serde_json::to_vec(self)?;
        std::fs::write(dir.join("zones.json"), data)?;
        Ok(())
    }

    pub fn load(table_path: &Path) -> Option<Self> {
        let path = table_path.join("pk_index/zones.json");
        let data = std::fs::read(&path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Mark row indices that may match CounterID = `counter` and EventDate in [min_date, max_date].
    pub fn apply_dashboard_prune(
        &self,
        mask: &mut [bool],
        counter: i32,
        min_date: i32,
        max_date: i32,
    ) {
        let row_count = mask.len();
        let mut row = 0usize;
        for zone in &self.zones {
            let zone_end = (row + ZONE_ROWS).min(row_count);
            let zone_len = zone_end.saturating_sub(row);
            if zone_len == 0 {
                break;
            }
            let might_match = zone.max_counter >= counter
                && zone.min_counter <= counter
                && zone.max_date >= min_date
                && zone.min_date <= max_date;
            if !might_match {
                for m in &mut mask[row..zone_end] {
                    *m = false;
                }
            }
            row = zone_end;
        }
    }
}

// keep binary path unused for now; json is fine for v0
#[allow(dead_code)]
fn _write_bin(path: &Path, zones: &[Zone]) -> Result<()> {
    let mut f = File::create(path)?;
    let n = zones.len() as u32;
    f.write_all(&n.to_le_bytes())?;
    for z in zones {
        f.write_all(&z.min_counter.to_le_bytes())?;
        f.write_all(&z.max_counter.to_le_bytes())?;
        f.write_all(&z.min_date.to_le_bytes())?;
        f.write_all(&z.max_date.to_le_bytes())?;
    }
    Ok(())
}

#[allow(dead_code)]
fn _read_bin(path: &Path) -> Result<ZoneIndex> {
    let mut f = File::open(path)?;
    let mut nbuf = [0u8; 4];
    f.read_exact(&mut nbuf)?;
    let n = u32::from_le_bytes(nbuf) as usize;
    let mut zones = Vec::with_capacity(n);
    for _ in 0..n {
        let mut b = [0u8; 4];
        f.read_exact(&mut b)?;
        let min_counter = i32::from_le_bytes(b);
        f.read_exact(&mut b)?;
        let max_counter = i32::from_le_bytes(b);
        f.read_exact(&mut b)?;
        let min_date = i32::from_le_bytes(b);
        f.read_exact(&mut b)?;
        let max_date = i32::from_le_bytes(b);
        zones.push(Zone {
            min_counter,
            max_counter,
            min_date,
            max_date,
        });
    }
    Ok(ZoneIndex { zones })
}
