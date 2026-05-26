use std::path::Path;

use rayon::prelude::*;
use arrow::array::{
    Array, Date32Array, Int16Array, Int32Array, Int64Array, LargeStringArray, StringArray,
    TimestampMicrosecondArray,
};
use arrow::datatypes::{DataType, TimeUnit};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use super::column::{ColumnData, ColumnType};
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

/// Infer all loadable columns from a Parquet file schema.
pub fn schema_from_parquet(parquet_path: impl AsRef<Path>) -> Result<Vec<(String, ColumnType)>> {
    let file = std::fs::File::open(parquet_path.as_ref())?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| crate::Error::msg(e.to_string()))?;
    let arrow_schema = builder.schema().clone();
    let mut cols = Vec::new();
    for field in arrow_schema.fields() {
        if let Some(ty) = arrow_type_to_column(field.data_type()) {
            cols.push((field.name().clone(), ty));
        }
    }
    if cols.is_empty() {
        return Err(crate::Error::msg("no supported columns in parquet file"));
    }
    Ok(cols)
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
    let col_schema = schema_from_parquet(parquet_path)?;
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
        (ColumnData::Int64(c), ColumnType::Int64) => {
            downcast_append(array, c, |a: &Int64Array, i| a.value(i))?;
        }
        (ColumnData::Int32(c), ColumnType::Int32) => {
            downcast_append(array, c, |a: &Int32Array, i| a.value(i))?;
        }
        (ColumnData::Int16(c), ColumnType::Int16) => {
            downcast_append(array, c, |a: &Int16Array, i| a.value(i))?;
        }
        (ColumnData::Date(c), ColumnType::Date) => {
            downcast_append(array, c, |a: &Date32Array, i| a.value(i))?;
        }
        (ColumnData::Timestamp(c), ColumnType::Timestamp) => match array.data_type() {
            DataType::Timestamp(TimeUnit::Microsecond, _) => {
                downcast_append(array, c, |a: &TimestampMicrosecondArray, i| a.value(i))?;
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
                        c.push(String::new());
                    } else {
                        c.push(a.value(i).to_string());
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
                        c.push(String::new());
                    } else {
                        c.push(a.value(i).to_string());
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

fn downcast_append<A: Array + 'static, T, F>(array: &dyn Array, out: &mut Vec<T>, f: F) -> Result<()>
where
    F: Fn(&A, usize) -> T,
{
    let a = array
        .as_any()
        .downcast_ref::<A>()
        .ok_or_else(|| crate::Error::msg("array downcast failed"))?;
    for i in 0..a.len() {
        if a.is_null(i) {
            return Err(crate::Error::msg("null value in hits dataset column"));
        }
        out.push(f(a, i));
    }
    Ok(())
}
