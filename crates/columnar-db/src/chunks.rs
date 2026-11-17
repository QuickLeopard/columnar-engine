//! The rows as the engine's chunks.
//!
//! **The database's typed values become the engine's own vocabulary here**,
//! and this is the only place that happens: a value is a `ScalarValue` on the
//! way in and a `ColumnChunk` on the way out, and the two are the same
//! vocabulary's two shapes rather than two vocabularies.
//!
//! **And the NULL is the engine's NULL.** The values array is dense and holds
//! a real value for every row -- the mask says which of them are real -- so a
//! NULL row's slot holds a placeholder. **The placeholder is never read**: the
//! mask is what the filter and the writer see, and the Parquet writer writes
//! the definition levels and not the value. The placeholder is `0`, `0.0` and
//! the empty string.

use columnar_engine::{ColumnChunk, DataType, Encoding, NullBitmap, ScalarValue};

use crate::error::{Error, Result};
use crate::row::Row;
use crate::schema::Schema;

/// Checks a row against the schema, and returns its values.
///
/// **Three things are checked, and all three are the schema's promise:** the
/// row holds one value per column; the value's type is the column's type;
/// and a column that may not hold a NULL does not hold one.
///
/// # Errors
///
/// Returns [`Error::RowLength`] when the row does not have one value per
/// column, [`Error::ValueType`] when a value's type is not the column's, and
/// [`Error::Null`] when a column that may not hold a NULL holds one.
pub(crate) fn checked_values(schema: &Schema, row: &Row) -> Result<Vec<Option<ScalarValue>>> {
    if row.len() != schema.len() {
        return Err(Error::RowLength { expected: schema.len(), got: row.len() });
    }
    let mut values = Vec::with_capacity(schema.len());
    for (index, column) in schema.columns().iter().enumerate() {
        let value = row.get(index).and_then(|value| value.as_ref());
        match value {
            None => {
                if !column.is_nullable() {
                    return Err(Error::Null { column: column.name().to_owned() });
                }
            }
            Some(value) => {
                if value.data_type() != column.data_type() {
                    return Err(Error::ValueType {
                        column: column.name().to_owned(),
                        expected: column.data_type().name().to_owned(),
                        got: value.data_type().name().to_owned(),
                    });
                }
            }
        }
        values.push(value.cloned());
    }
    Ok(values)
}

/// The engine's chunks for the rows: one chunk per column.
///
/// # Errors
///
/// Returns the same errors as [`checked_values`], because the rows are
/// checked again here: **a row is checked when it is offered and checked
/// again when it is written**, and a check that only happened once would be a
/// check that could be forgotten.
pub(crate) fn chunks_of(schema: &Schema, rows: &[Row]) -> Result<Vec<ColumnChunk>> {
    let mut columns: Vec<Vec<Option<ScalarValue>>> = Vec::with_capacity(schema.len());
    for _ in schema.columns() {
        columns.push(Vec::with_capacity(rows.len()));
    }
    for row in rows {
        let values = checked_values(schema, row)?;
        for (column, value) in columns.iter_mut().zip(values) {
            column.push(value);
        }
    }

    let mut chunks = Vec::with_capacity(schema.len());
    for (column, values) in schema.columns().iter().zip(&columns) {
        let nulls: Vec<usize> = values
            .iter()
            .enumerate()
            .filter_map(|(row, value)| value.is_none().then_some(row))
            .collect();
        let validity = NullBitmap::with_nulls(rows.len(), &nulls);
        let encoding = match column.data_type() {
            DataType::Int64 => Encoding::Int64 {
                values: values
                    .iter()
                    .map(|value| int64_of(column.name(), value.as_ref()))
                    .collect::<Result<Vec<i64>>>()?,
            },
            DataType::Float64 => Encoding::Float64 {
                values: values
                    .iter()
                    .map(|value| float64_of(column.name(), value.as_ref()))
                    .collect::<Result<Vec<f64>>>()?,
            },
            DataType::Utf8 => Encoding::Utf8 {
                values: values
                    .iter()
                    .map(|value| utf8_of(column.name(), value.as_ref()))
                    .collect::<Result<Vec<String>>>()?,
            },
        };
        chunks.push(ColumnChunk::new(column.name(), encoding, validity));
    }
    Ok(chunks)
}

/// The integer a value holds, or the placeholder for a NULL.
fn int64_of(column: &str, value: Option<&ScalarValue>) -> Result<i64> {
    match value {
        Some(ScalarValue::Int64(number)) => Ok(*number),
        None => Ok(0),
        Some(other) => Err(Error::ValueType {
            column: column.to_owned(),
            expected: DataType::Int64.name().to_owned(),
            got: other.data_type().name().to_owned(),
        }),
    }
}

/// The double a value holds, or the placeholder for a NULL.
fn float64_of(column: &str, value: Option<&ScalarValue>) -> Result<f64> {
    match value {
        Some(ScalarValue::Float64(number)) => Ok(*number),
        None => Ok(0.0),
        Some(other) => Err(Error::ValueType {
            column: column.to_owned(),
            expected: DataType::Float64.name().to_owned(),
            got: other.data_type().name().to_owned(),
        }),
    }
}

/// The string a value holds, or the placeholder for a NULL.
fn utf8_of(column: &str, value: Option<&ScalarValue>) -> Result<String> {
    match value {
        Some(ScalarValue::Utf8(text)) => Ok(text.clone()),
        None => Ok(String::new()),
        Some(other) => Err(Error::ValueType {
            column: column.to_owned(),
            expected: DataType::Utf8.name().to_owned(),
            got: other.data_type().name().to_owned(),
        }),
    }
}
