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
//!
//! **And the file carries the database's own columns beside the
//! application's.** The hidden `row_id` and `deleted` columns are written
//! with every row, so a version's identity and a tombstone travel with the
//! values they belong to -- **and a tombstone is a row like any other**, with
//! no value in the application's columns and the id of the version it retires
//! in its `row_id`.
//!
//! **The hidden columns are always plain and never NULL**, so every file of a
//! table agrees about them: the engine reads a set of files as one table, and
//! a set of files has to agree about its columns.

use columnar_engine::{ColumnChunk, DataType, Encoding, NullBitmap, ScalarValue};

use crate::error::{Error, Result};
use crate::schema::{Column, Schema};
use crate::versions::{Stored, DELETED, LIVE, ROW_ID, TOMBSTONE};

/// Checks a row's values against the schema, and returns them.
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
pub(crate) fn checked_values(
    schema: &Schema,
    values: &[Option<ScalarValue>],
) -> Result<Vec<Option<ScalarValue>>> {
    if values.len() != schema.len() {
        return Err(Error::RowLength { expected: schema.len(), got: values.len() });
    }
    let mut checked = Vec::with_capacity(schema.len());
    for (index, column) in schema.columns().iter().enumerate() {
        let value = values.get(index).and_then(|value| value.as_ref());
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
        checked.push(value.cloned());
    }
    Ok(checked)
}

/// The optionality of the file a table's rows are written into.
///
/// **The application's columns are optional, and the hidden ones are required — and the tombstone
/// is the reason for both.** A tombstone holds no value of the application's, so the application's
/// columns carry an absent value in every file that holds one, and a column that may hold an
/// absent value is written optional. **So the optionality is the database's row model's and not the
/// rows' happenstance**: it is the same in every file of every table, it is decided before the first
/// row is written, and the files of a table agree about it.
///
/// **And the hidden columns are required**, because they never hold an absent value: the id is the
/// version's own and the flag says whether the row is a tombstone.
///
/// **And the table's schema's nullability is the application's promise, and it is kept at the
/// boundary**: the database refuses a NULL in a column the schema says may not hold one, and the
/// catalog says which columns may. **The file says what the database's rows need, and not what the
/// application promised** — and the two are different things on purpose.
///
/// **And the order is the chunks' order**: the application's columns in the schema's order, and
/// then the two hidden columns.
pub(crate) fn optionality_of(schema: &Schema) -> Vec<bool> {
    let mut flags = vec![true; schema.len()];
    flags.push(false);
    flags.push(false);
    flags
}

/// The engine's chunks for the rows: one chunk per column.
///
/// # Errors
///
/// Returns the same errors as [`checked_values`], because the rows are
/// checked again here: **a row is checked when it is offered and checked
/// again when it is written**, and a check that only happened once would be a
/// check that could be forgotten.
pub(crate) fn chunks_of(schema: &Schema, rows: &[Stored]) -> Result<Vec<ColumnChunk>> {
    let mut columns: Vec<Vec<Option<ScalarValue>>> = Vec::with_capacity(schema.len());
    for _ in schema.columns() {
        columns.push(Vec::with_capacity(rows.len()));
    }
    for row in rows {
        if row.deleted {
            // **A tombstone holds no value of the application's.** It is the
            // database's own row, and the application's columns are empty in
            // it -- which is what makes it a tombstone and not a version.
            for column in &mut columns {
                column.push(None);
            }
        } else {
            let values = checked_values(schema, &row.values)?;
            for (column, value) in columns.iter_mut().zip(values) {
                column.push(value);
            }
        }
    }

    let mut chunks = Vec::with_capacity(schema.len() + 2);
    for (column, values) in schema.columns().iter().zip(&columns) {
        chunks.push(chunk_of(column, values)?);
    }

    // **The hidden columns.** The id is the version's own, and the flag says
    // whether the row is a tombstone.
    let ids: Vec<Option<ScalarValue>> =
        rows.iter().map(|row| Some(ScalarValue::Int64(row.row_id))).collect();
    let flags: Vec<Option<ScalarValue>> = rows
        .iter()
        .map(|row| {
            let flag = if row.deleted { TOMBSTONE } else { LIVE };
            Some(ScalarValue::Int64(flag))
        })
        .collect();
    chunks.push(chunk_of(&Column::new(ROW_ID, DataType::Int64, false), &ids)?);
    chunks.push(chunk_of(&Column::new(DELETED, DataType::Int64, false), &flags)?);
    Ok(chunks)
}

/// The chunk for one column's values.
fn chunk_of(column: &Column, values: &[Option<ScalarValue>]) -> Result<ColumnChunk> {
    let nulls: Vec<usize> = values
        .iter()
        .enumerate()
        .filter_map(|(row, value)| value.is_none().then_some(row))
        .collect();
    let validity = NullBitmap::with_nulls(values.len(), &nulls);
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
    Ok(ColumnChunk::new(column.name(), encoding, validity))
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
