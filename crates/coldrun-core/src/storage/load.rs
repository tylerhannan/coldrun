use std::path::Path;

use rayon::prelude::*;
use arrow::array::{
    Array, Date32Array, Int16Array, Int32Array, Int64Array, LargeStringArray, StringArray,
    TimestampMicrosecondArray, UInt8Array, UInt16Array, UInt32Array,
};
use arrow::datatypes::{DataType, TimeUnit};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use super::column::{ColumnData, ColumnType};
use super::pod::PodStorage;
use super::table::{ColumnMeta, Table};
use super::Database;
use crate::Result;

/// Columns needed for early ClickBench queries (demo / partial loads).
pub fn hits_column_schema() -> Vec<(&'static str, ColumnType)> {
    vec![
        ("WatchID", ColumnType::Int64),
        ("JavaEnable", ColumnType::Int16),
        ("Title", ColumnType::Utf8),
        ("GoodEvent", ColumnType::Int16),
        ("EventTime", ColumnType::Timestamp),
        ("EventDate", ColumnType::Date),
        ("CounterID", ColumnType::Int32),
        ("ClientIP", ColumnType::Int32),
        ("RegionID", ColumnType::Int32),
        ("UserID", ColumnType::Int64),
        ("AdvEngineID", ColumnType::Int16),
        ("ResolutionWidth", ColumnType::Int16),
        ("SearchPhrase", ColumnType::Utf8),
        ("URL", ColumnType::Utf8),
        ("Referer", ColumnType::Utf8),
        ("MobilePhoneModel", ColumnType::Utf8),
        ("MobilePhone", ColumnType::Int16),
        ("SearchEngineID", ColumnType::Int16),
        ("IsRefresh", ColumnType::Int16),
        ("TraficSourceID", ColumnType::Int16),
        ("DontCountHits", ColumnType::Int16),
        ("IsLink", ColumnType::Int16),
        ("IsDownload", ColumnType::Int16),
        ("URLHash", ColumnType::Int64),
        ("RefererHash", ColumnType::Int64),
        ("WindowClientWidth", ColumnType::Int16),
        ("WindowClientHeight", ColumnType::Int16),
    ]
}

/// Infer all loadable columns from a Parquet file schema (any supported Arrow type).
#[allow(dead_code)]
pub fn schema_from_parquet(parquet_path: impl AsRef<Path>) -> Result<Vec<(String, ColumnType)>> {
    let fields = parquet_field_types(parquet_path.as_ref())?;
    let mut cols = Vec::new();
    for (name, dt) in &fields {
        if let Some(ty) = logical_column_type(name, dt) {
            cols.push((name.clone(), ty));
        }
    }
    if cols.is_empty() {
        return Err(crate::Error::msg("no supported columns in parquet file"));
    }
    Ok(cols)
}

/// ClickBench `hits` columns present in the file (subset of full parquet schema).
pub fn clickbench_parquet_schema(parquet_path: impl AsRef<Path>) -> Result<Vec<(String, ColumnType)>> {
    let fields = parquet_field_types(parquet_path.as_ref())?;
    let mut cols = Vec::new();
    for (name, want_ty) in hits_column_schema() {
        let Some(dt) = fields.get(name) else {
            continue;
        };
        let Some(ty) = logical_column_type(name, dt) else {
            continue;
        };
        if ty == want_ty {
            cols.push((name.to_string(), ty));
        }
    }
    if cols.is_empty() {
        return Err(crate::Error::msg(
            "parquet file has none of the ClickBench hits columns coldrun needs",
        ));
    }
    Ok(cols)
}

fn parquet_field_types(path: &Path) -> Result<std::collections::HashMap<String, DataType>> {
    let file = std::fs::File::open(path)?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| crate::Error::msg(e.to_string()))?;
    let mut map = std::collections::HashMap::new();
    for field in builder.schema().fields() {
        map.insert(field.name().clone(), field.data_type().clone());
    }
    Ok(map)
}

/// Map on-disk ClickBench-compatible types to coldrun column types.
fn logical_column_type(name: &str, dt: &DataType) -> Option<ColumnType> {
    match name {
        "EventTime" => Some(ColumnType::Timestamp),
        "EventDate" => Some(ColumnType::Date),
        _ => arrow_type_to_column(dt),
    }
}

fn arrow_type_to_column(dt: &DataType) -> Option<ColumnType> {
    match dt {
        DataType::Int64 => Some(ColumnType::Int64),
        DataType::Int32 | DataType::UInt32 => Some(ColumnType::Int32),
        DataType::Int16 | DataType::UInt16 | DataType::Int8 | DataType::UInt8 => Some(ColumnType::Int16),
        DataType::Date32 => Some(ColumnType::Date),
        DataType::Timestamp(_, _) => Some(ColumnType::Timestamp),
        DataType::Utf8 | DataType::LargeUtf8 => Some(ColumnType::Utf8),
        _ => None,
    }
}

pub fn load_parquet_into_table(
    db: &mut Database,
    table_name: &str,
    parquet_path: impl AsRef<Path>,
) -> Result<u64> {
    let parquet_path = parquet_path.as_ref();
    let col_schema = clickbench_parquet_schema(parquet_path)?;
    load_parquet_columns(db, table_name, parquet_path, &col_schema)
}

pub fn load_parquet_columns(
    db: &mut Database,
    table_name: &str,
    parquet_path: &Path,
    col_schema: &[(String, ColumnType)],
) -> Result<u64> {
    let table_dir = db.table_path(table_name);
    if table_dir.exists() {
        std::fs::remove_dir_all(&table_dir)?;
    }

    let meta_cols: Vec<ColumnMeta> = col_schema
        .iter()
        .map(|(n, ty)| ColumnMeta {
            name: n.clone(),
            ty: *ty,
        })
        .collect();

    let mut table = Table::create(&table_dir, table_name, meta_cols)?;
    for (name, ty) in col_schema {
        table.ensure_column(name, *ty)?;
    }

    let file = std::fs::File::open(parquet_path)?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| crate::Error::msg(e.to_string()))?;
    let reader = builder.build().map_err(|e| crate::Error::msg(e.to_string()))?;

    let mut total_rows = 0u64;
    for batch in reader {
        let batch = batch.map_err(|e| crate::Error::msg(e.to_string()))?;
        let n = batch.num_rows();
        total_rows += n as u64;

        let chunks: Vec<(String, ColumnData)> = col_schema
            .par_iter()
            .map(|(col_name, col_ty)| {
                let array = batch.column_by_name(col_name).ok_or_else(|| {
                    crate::Error::msg(format!("parquet batch missing column {col_name}"))
                })?;
                let mut chunk = super::column::empty_column(*col_ty);
                append_array(&mut chunk, array.as_ref(), *col_ty)?;
                Ok((col_name.clone(), chunk))
            })
            .collect::<Result<Vec<_>>>()?;

        for (col_name, chunk) in chunks {
            table.column_mut(&col_name)?.extend_from(&chunk)?;
        }
    }

    table.set_row_count(total_rows);
    table.flush()?;
    db.register_table(table_name)?;
    Ok(total_rows)
}

fn append_array(col: &mut ColumnData, array: &dyn Array, ty: ColumnType) -> Result<()> {
    match (col, ty) {
        (ColumnData::Int64(c), ColumnType::Int64) => match array.data_type() {
            DataType::Int64 => {
                downcast_append_pod(array, c, |a: &Int64Array, i| a.value(i), 0i64)?;
            }
            other => {
                return Err(crate::Error::msg(format!("unsupported int64 type: {other:?}")));
            }
        },
        (ColumnData::Int32(c), ColumnType::Int32) => match array.data_type() {
            DataType::Int32 => {
                downcast_append_pod(array, c, |a: &Int32Array, i| a.value(i), 0i32)?;
            }
            DataType::UInt32 => {
                downcast_append_pod(array, c, |a: &UInt32Array, i| a.value(i) as i32, 0i32)?;
            }
            other => {
                return Err(crate::Error::msg(format!("unsupported int32 type: {other:?}")));
            }
        },
        (ColumnData::Int16(c), ColumnType::Int16) => match array.data_type() {
            DataType::Int16 => {
                downcast_append_pod(array, c, |a: &Int16Array, i| a.value(i), 0i16)?;
            }
            DataType::UInt16 => {
                downcast_append_pod(array, c, |a: &UInt16Array, i| a.value(i) as i16, 0i16)?;
            }
            DataType::UInt8 | DataType::Int8 => {
                downcast_append_pod(array, c, |a: &UInt8Array, i| a.value(i) as i16, 0i16)?;
            }
            other => {
                return Err(crate::Error::msg(format!("unsupported int16 type: {other:?}")));
            }
        },
        (ColumnData::Date(c), ColumnType::Date) => match array.data_type() {
            DataType::Date32 => {
                downcast_append_pod(array, c, |a: &Date32Array, i| a.value(i), 0i32)?;
            }
            DataType::UInt16 => {
                downcast_append_pod(array, c, |a: &UInt16Array, i| i32::from(a.value(i)), 0i32)?;
            }
            DataType::Int32 => {
                downcast_append_pod(array, c, |a: &Int32Array, i| a.value(i), 0i32)?;
            }
            other => {
                return Err(crate::Error::msg(format!("unsupported date type: {other:?}")));
            }
        },
        (ColumnData::Timestamp(c), ColumnType::Timestamp) => match array.data_type() {
            DataType::Timestamp(TimeUnit::Microsecond, _) => {
                downcast_append_pod(array, c, |a: &TimestampMicrosecondArray, i| a.value(i), 0i64)?;
            }
            DataType::Int64 => {
                let a = array
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| crate::Error::msg("int64 timestamp downcast failed"))?;
                for i in 0..a.len() {
                    let v = if a.is_null(i) { 0 } else { a.value(i) };
                    let micros = event_time_to_micros(v);
                    c.push(micros)?;
                }
            }
            other => {
                return Err(crate::Error::msg(format!(
                    "unsupported timestamp type: {other:?}"
                )));
            }
        },
        (ColumnData::Utf8(c), ColumnType::Utf8) => match array.data_type() {
            DataType::Utf8 => {
                let a = array
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| crate::Error::msg("utf8 downcast failed"))?;
                for i in 0..a.len() {
                    if a.is_null(i) {
                        c.push_str("");
                    } else {
                        c.push_str(a.value(i));
                    }
                }
            }
            DataType::LargeUtf8 => {
                let a = array
                    .as_any()
                    .downcast_ref::<LargeStringArray>()
                    .ok_or_else(|| crate::Error::msg("large utf8 downcast failed"))?;
                for i in 0..a.len() {
                    if a.is_null(i) {
                        c.push_str("");
                    } else {
                        c.push_str(a.value(i));
                    }
                }
            }
            other => {
                return Err(crate::Error::msg(format!("unsupported string type: {other:?}")));
            }
        },
        _ => return Err(crate::Error::msg("column type mismatch during load")),
    }
    Ok(())
}

/// ClickBench `hits_compatible` stores EventTime as Unix seconds in Int64.
fn event_time_to_micros(v: i64) -> i64 {
    if v.abs() < 10_000_000_000_000 {
        v.saturating_mul(1_000_000)
    } else {
        v
    }
}

fn downcast_append_pod<A: Array + 'static, T: Copy, F>(
    array: &dyn Array,
    out: &mut PodStorage<T>,
    f: F,
    null_default: T,
) -> Result<()>
where
    F: Fn(&A, usize) -> T,
{
    let a = array
        .as_any()
        .downcast_ref::<A>()
        .ok_or_else(|| crate::Error::msg("array downcast failed"))?;
    for i in 0..a.len() {
        let v = if a.is_null(i) { null_default } else { f(a, i) };
        out.push(v)?;
    }
    Ok(())
}
