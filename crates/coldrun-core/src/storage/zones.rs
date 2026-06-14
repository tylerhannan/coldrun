use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::column::ColumnData;
use super::table::Table;
use crate::Result;

pub const ZONE_ROWS: usize = 8192;
pub const ZONE_VERSION_V1: u32 = 1;
pub const ZONE_VERSION_V2: u32 = 2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Zone {
    pub min_counter: i32,
    pub max_counter: i32,
    pub min_date: i32,
    pub max_date: i32,
    /// Pre-aggregated nonzero `AdvEngineID` rows in this zone (v1 index only).
    #[serde(default)]
    pub adv_nonzero: u32,
    #[serde(default = "adv_min_unknown")]
    pub min_adv: i16,
    #[serde(default = "adv_max_unknown")]
    pub max_adv: i16,
    /// EventTime bounds per zone (v2) for ORDER BY / time-range pruning.
    #[serde(default = "event_time_min_unknown")]
    pub min_event_time: i64,
    #[serde(default = "event_time_max_unknown")]
    pub max_event_time: i64,
}

fn event_time_min_unknown() -> i64 {
    i64::MAX
}

fn event_time_max_unknown() -> i64 {
    i64::MIN
}

fn adv_min_unknown() -> i16 {
    i16::MIN
}

fn adv_max_unknown() -> i16 {
    i16::MAX
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneIndex {
    #[serde(default)]
    pub version: u32,
    pub zones: Vec<Zone>,
}

impl ZoneIndex {
    pub fn build(table: &Table) -> Option<Self> {
        let counter = table.column("CounterID").ok()?;
        let date = table.column("EventDate").ok()?;
        let (ColumnData::Int32(counters), ColumnData::Date(dates)) = (counter, date) else {
            return None;
        };
        let adv = table.column("AdvEngineID").ok();
        let event_time = table.column("EventTime").ok();
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
            let (min_adv, max_adv, adv_nonzero) = if let Some(ColumnData::Int16(adv_col)) = adv {
                let slice_a = &adv_col[z..end];
                let mut nz = 0u32;
                for &x in slice_a {
                    if x != 0 {
                        nz += 1;
                    }
                }
                (
                    *slice_a.iter().min().unwrap_or(&0),
                    *slice_a.iter().max().unwrap_or(&0),
                    nz,
                )
            } else {
                (0, 0, 0)
            };
            let (min_event_time, max_event_time) =
                if let Some(ColumnData::Timestamp(ts_col)) = event_time {
                    let slice_t = &ts_col[z..end.min(ts_col.len())];
                    if slice_t.is_empty() {
                        (i64::MAX, i64::MIN)
                    } else {
                        (
                            *slice_t.iter().min().unwrap_or(&i64::MAX),
                            *slice_t.iter().max().unwrap_or(&i64::MIN),
                        )
                    }
                } else {
                    (i64::MAX, i64::MIN)
                };
            zones.push(Zone {
                min_counter: *slice_c.iter().min().unwrap_or(&0),
                max_counter: *slice_c.iter().max().unwrap_or(&0),
                min_date: *slice_d.iter().min().unwrap_or(&0),
                max_date: *slice_d.iter().max().unwrap_or(&0),
                adv_nonzero,
                min_adv,
                max_adv,
                min_event_time,
                max_event_time,
            });
            z = end;
        }
        let version = if event_time.is_some() {
            ZONE_VERSION_V2
        } else if adv.is_some() {
            ZONE_VERSION_V1
        } else {
            0
        };
        Some(Self { version, zones })
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

    /// Sum pre-aggregated `AdvEngineID <> 0` row counts (v1 zones).
    pub fn count_adv_nonzero_total(&self) -> Option<u64> {
        if self.version < ZONE_VERSION_V1 {
            return None;
        }
        Some(
            self.zones
                .iter()
                .map(|z| u64::from(z.adv_nonzero))
                .sum(),
        )
    }

    /// Row ranges `[start, end)` whose zone min/max may contain the dashboard PK filter.
    pub fn dashboard_matching_ranges(
        &self,
        row_count: usize,
        counter: i32,
        min_date: i32,
        max_date: i32,
    ) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut row = 0usize;
        for zone in &self.zones {
            let zone_end = (row + ZONE_ROWS).min(row_count);
            if zone_end <= row {
                break;
            }
            if zone.max_counter >= counter
                && zone.min_counter <= counter
                && zone.max_date >= min_date
                && zone.min_date <= max_date
            {
                ranges.push((row, zone_end));
            }
            row = zone_end;
        }
        ranges
    }

    /// Build a sparse mask: only PK-matching dashboard zones are true (faster than all-true + prune).
    pub fn build_sparse_dashboard_mask(
        &self,
        row_count: usize,
        counter: i32,
        min_date: i32,
        max_date: i32,
    ) -> Vec<bool> {
        let mut mask = vec![false; row_count];
        let mut row = 0usize;
        for zone in &self.zones {
            let zone_end = (row + ZONE_ROWS).min(row_count);
            if zone_end <= row {
                break;
            }
            if zone.max_counter >= counter
                && zone.min_counter <= counter
                && zone.max_date >= min_date
                && zone.min_date <= max_date
            {
                for m in &mut mask[row..zone_end] {
                    *m = true;
                }
            }
            row = zone_end;
        }
        mask
    }

    /// Clear mask bits in zones that cannot contain `AdvEngineID <> 0`.
    pub fn apply_adv_ne_zero_prune(&self, mask: &mut [bool]) {
        if self.version < ZONE_VERSION_V1 {
            return;
        }
        let row_count = mask.len();
        let mut row = 0usize;
        for zone in &self.zones {
            let zone_end = (row + ZONE_ROWS).min(row_count);
            if zone_len_zero(zone_end, row) {
                break;
            }
            if zone.max_adv <= 0 {
                for m in &mut mask[row..zone_end] {
                    *m = false;
                }
            }
            row = zone_end;
        }
    }

    /// True when zone EventTime bounds are non-decreasing in physical row order (demo / sorted loads).
    pub fn event_time_monotonic_in_row_order(&self) -> bool {
        if self.version < ZONE_VERSION_V2 || self.zones.is_empty() {
            return false;
        }
        self.zones
            .windows(2)
            .all(|w| w[0].max_event_time <= w[1].min_event_time)
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
            if zone_len_zero(zone_end, row) {
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
            adv_nonzero: 0,
            min_adv: 0,
            max_adv: 0,
            min_event_time: i64::MAX,
            max_event_time: i64::MIN,
        });
    }
    Ok(ZoneIndex {
        version: ZONE_VERSION_V1,
        zones,
    })
}

fn zone_len_zero(zone_end: usize, row: usize) -> bool {
    zone_end.saturating_sub(row) == 0
}
