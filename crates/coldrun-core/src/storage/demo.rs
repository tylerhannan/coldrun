use chrono::{TimeZone, Utc};

use super::column::{ColumnData, ColumnType};
use super::load::hits_column_schema;
use super::table::{ColumnMeta, Table};
use super::Database;
use crate::Result;

const TARGET_USER: i64 = 435_090_932_899_640_449;
const JULY1_DAYS: i32 = 15964; // 2013-07-01 since epoch
const URL_HASH_TARGET: i64 = 2_868_770_270_353_813_622;
const REFERER_HASH_TARGET: i64 = 3_594_120_000_172_545_465;

/// Insert synthetic `hits` for local dev (not for benchmark scores).
pub fn load_demo_hits(db: &mut Database, rows: u64) -> Result<u64> {
    let table_name = "hits";
    let table_dir = db.table_path(table_name);
    if table_dir.exists() {
        std::fs::remove_dir_all(&table_dir)?;
    }

    let col_schema = hits_column_schema();
    let meta_cols: Vec<ColumnMeta> = col_schema
        .iter()
        .map(|(n, ty)| ColumnMeta {
            name: (*n).to_string(),
            ty: *ty,
        })
        .collect();

    let mut table = Table::create(&table_dir, table_name, meta_cols)?;
    for (name, ty) in &col_schema {
        table.ensure_column(name, *ty)?;
    }

    let n = rows as usize;
    let base_ts = Utc
        .with_ymd_and_hms(2013, 7, 1, 0, 0, 0)
        .unwrap()
        .timestamp_micros();

    for (name, ty) in &col_schema {
        let col = table.column_mut(name)?;
        match (col, ty) {
            (ColumnData::Int64(c), ColumnType::Int64) => {
                for i in 0..n {
                    let v = if *name == "UserID" && i == 42 {
                        TARGET_USER
                    } else if *name == "WatchID" {
                        i as i64
                    } else if *name == "URLHash" && i % 5000 == 0 {
                        URL_HASH_TARGET
                    } else if *name == "RefererHash" && i % 7000 == 0 {
                        REFERER_HASH_TARGET
                    } else {
                        i as i64
                    };
                    let _ = c.push(v);
                }
            }
            (ColumnData::Int32(c), ColumnType::Int32) => {
                for i in 0..n {
                    let v = match *name {
                        "CounterID" if i % 100 == 0 => 62,
                        "RegionID" => (i % 1000) as i32,
                        "ClientIP" => (i as i32).wrapping_mul(7),
                        _ => (i % 500) as i32,
                    };
                    let _ = c.push(v);
                }
            }
            (ColumnData::Int16(c), ColumnType::Int16) => {
                for i in 0..n {
                    let v = match *name {
                        "AdvEngineID" => (i % 8) as i16,
                        "TraficSourceID" if i % 3000 == 0 => -1,
                        "TraficSourceID" if i % 4000 == 0 => 6,
                        "TraficSourceID" => (i % 20) as i16,
                        "DontCountHits" => if i % 3 == 0 { 1 } else { 0 },
                        "IsRefresh" => if i % 4 == 0 { 1 } else { 0 },
                        "IsLink" => if i % 5 == 0 { 1 } else { 0 },
                        "IsDownload" => 0,
                        "MobilePhone" => (i % 10) as i16,
                        "SearchEngineID" => (i % 15) as i16,
                        "ResolutionWidth" => (i % 100) as i16,
                        "WindowClientWidth" => (i % 200) as i16,
                        "WindowClientHeight" => (i % 150) as i16,
                        _ => (i % 7) as i16,
                    };
                    let _ = c.push(v);
                }
            }
            (ColumnData::Date(c), ColumnType::Date) => {
                for i in 0..n {
                    let day = JULY1_DAYS + (i % 31) as i32;
                    let _ = c.push(day);
                }
            }
            (ColumnData::Timestamp(c), ColumnType::Timestamp) => {
                for i in 0..n {
                    let _ = c.push(base_ts + (i as i64) * 60_000_000);
                }
            }
            (ColumnData::Utf8(c), ColumnType::Utf8) => {
                for i in 0..n {
                    let v = match *name {
                        "MobilePhoneModel" if i % 5 == 0 => String::new(),
                        "SearchPhrase" if i % 3 == 0 => String::new(),
                        "URL" if i % 10 == 0 => format!("https://www.google.com/search?q={i}"),
                        "URL" if i % 17 == 0 => format!("https://example.com/page/{i}"),
                        "Title" if i % 7 == 0 => "Google Search Results".into(),
                        "Referer" if i % 11 == 0 => {
                            format!("https://www.google.com/referer/{i}")
                        }
                        "Referer" if i % 13 == 0 => format!("https://news.ycombinator.com/item?id={i}"),
                        _ => format!("value-{i}"),
                    };
                    let _ = c.push(v);
                }
            }
            _ => {}
        }
    }

    table.set_row_count(rows);
    table.meta.demo_near_unique = true;
    table.flush()?;
    db.register_table(table_name)?;
    Ok(rows)
}
