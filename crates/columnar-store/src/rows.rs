//! The engine's rows, in the store's own terms.
//!
//! The engine holds its rows in **its own** vocabulary: a `ColumnChunk` with
//! an encoding and a validity mask. The Parquet writer holds them in
//! **Arrow's**: a `RecordBatch` of arrays. **Somebody has to turn one into
//! the other**, and this module is where it happens -- in the store, because
//! the conversion cannot live in `columnar-core` (a vocabulary that depends
//! on a framework is not a vocabulary) and must not live in the engine (the
//! engine's surface would carry Arrow).
//!
//! # What the conversion is, value for value
//!
//! | the engine's chunk | Arrow's array |
//! |---|---|
//! | `Int64 { values }` | `Int64Array` |
//! | `Float64 { values }` | `Float64Array` |
//! | `Utf8 { values }` | `StringArray` |
//! | `Int64Dict { dict, indices }` | a `DictionaryArray` over `Int64` |
//! | `Float64Dict { dict, indices }` | a `DictionaryArray` over `Float64` |
//! | `Utf8Dict { dict, indices }` | a `DictionaryArray` over `Utf8` |
//! | `BoolRle { runs }` | `Int64Array` of zeros and ones |
//!
//! The validity mask becomes Arrow's null buffer, and the mask's words are
//! laid out the way Arrow's are: LSB first, within the word and within the
//! byte.
//!
//! **The dictionary the engine built does not decide the file.** The Parquet
//! writer interns the values again, per row group, and the first appearance
//! order is the rows' own order -- so a file-wide dictionary and a page's
//! dictionary write the same bytes. The measurement is in spec 042.
//!
//! # The schema is the caller's
//!
//! The engine's rows say which rows are NULL. They do **not** say whether a
//! column may be NULL, and the Parquet writer makes that difference visible
//! in the bytes: an optional column carries definition levels and a required
//! one does not.
//!
//! **So the optionality is the caller's, and the store does not derive it.**
//! The caller says which columns may hold a NULL, and the store writes the
//! file that says so. **The rule the store used to apply — a column that
//! holds no NULL is written REQUIRED, and a column that holds one is written
//! OPTIONAL — is still available, and it is named**: it is
//! [`nullability`], and it is a choice the caller takes rather than a rule
//! the store applies.
//!
//! **And the reason the store stopped deciding is the streaming write.** A
//! writer's schema is fixed before the first row, and the rows' own shape is
//! known only after the last one, so a schema derived from the data cannot
//! be used by a write that streams. **And the reason it matters beyond the
//! streaming write is that the rows' happenstance is not a promise**: two
//! files of one table can hold different optionality for the same column,
//! because one of them held a NULL and the other did not. **So the caller
//! decides, and the files of a table agree** — which is the property the
//! derivation could not promise, and the reason the decision moved out of the
//! store.

use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, DictionaryArray, Float64Array, Int32Array, Int64Array, StringArray,
};
use arrow::buffer::{BooleanBuffer, Buffer, NullBuffer, ScalarBuffer};
use arrow::datatypes::{DataType, Field, Fields, Int32Type, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use columnar_core::{ColumnChunk, Encoding, NullBitmap};

use crate::error::{Error, Result};
use crate::writer::BATCH_ROWS;

/// The rows' own optionality: a column that holds a NULL is optional.
///
/// **This is the rule the store used to apply, and it is a choice the caller
/// takes now.** A caller that has a schema of its own — a table's schema, say
/// — states the optionality itself, and the file agrees with the promise. A
/// caller that has only the rows takes this one, and the file says what the
/// rows happened to be.
///
/// **And it is one flag a column**, in the columns' own order, so it lines up
/// with the rows it is about.
#[must_use]
pub fn nullability(rows: &[ColumnChunk]) -> Vec<bool> {
    rows.iter().map(|row| row.validity.null_count() > 0).collect()
}

/// The engine's rows, as Arrow's arrays.
///
/// The arrays are the **whole** column: the rows are re-batched afterwards,
/// and a batch is a slice of an array.
///
/// # Errors
///
/// Returns [`crate::Error::Invalid`] when a chunk holds a dictionary index
/// that is out of range for its dictionary, which is a corrupt chunk rather
/// than a value.
pub fn arrays(rows: &[ColumnChunk]) -> Result<Vec<ArrayRef>> {
    rows.iter().map(array_of).collect()
}

/// The Arrow schema of the engine's rows.
///
/// **The optionality is the caller's**, one flag a column, in the columns'
/// own order: a column that may hold a NULL is written OPTIONAL, and one that
/// may not is written REQUIRED. **The dictionary columns are
/// `Dictionary(Int32, …)`, because that is what the Parquet writer writes
/// them as.**
///
/// # Errors
///
/// Returns [`Error::Optionality`] when the rows and the optionality do not
/// have the same number of columns, because a schema built from one and a
/// flag list built from the other would be a file whose columns and fields
/// disagree.
pub fn schema(rows: &[ColumnChunk], nullable: &[bool]) -> Result<SchemaRef> {
    check_optionality(rows.len(), nullable.len())?;
    let fields: Vec<Field> = rows
        .iter()
        .zip(nullable)
        .map(|(chunk, may_hold_null)| {
            Field::new(&chunk.name, data_type(&chunk.encoding), *may_hold_null)
        })
        .collect();
    Ok(Arc::new(Schema::new(Fields::from(fields))))
}

/// Checks that the rows and the optionality agree about the columns.
///
/// **The two are one thing said twice** — the rows name the columns and the
/// flags say which of them may hold a NULL — and a caller that says them
/// differently has said nothing.
fn check_optionality(columns: usize, optionality: usize) -> Result<()> {
    if columns == optionality {
        return Ok(());
    }
    Err(Error::Optionality { columns, optionality })
}

/// The engine's rows, in the store's grid.
///
/// The grid is the store's rule and the store keeps it: the rows are
/// re-batched here, inside the store, and a batch that cannot be on the grid
/// is still an error.
///
/// **And the optionality is the caller's**, one flag a column: every batch
/// carries the schema the caller stated, so a batch that holds no NULL in an
/// optional column is still written under an optional field.
pub fn batches(rows: &[ColumnChunk], nullable: &[bool]) -> Result<Vec<RecordBatch>> {
    let schema = schema(rows, nullable)?;
    let arrays = arrays(rows)?;
    let count = rows.first().map_or(0, |chunk| chunk.len);
    let mut batches = Vec::with_capacity(count.div_ceil(BATCH_ROWS));
    let mut start = 0;
    while start < count {
        let len = BATCH_ROWS.min(count - start);
        let columns = arrays.iter().map(|array| array.slice(start, len)).collect();
        batches.push(
            RecordBatch::try_new(schema.clone(), columns)
                .map_err(|error| Error::Parquet(error.to_string()))?,
        );
        start += len;
    }
    Ok(batches)
}

/// The Arrow type of an encoding.
///
/// The dictionary is an **encoding**, not a type: a dictionary-encoded column
/// is still a column of its own values, and the Parquet writer writes it as
/// a `Dictionary` so that it can dictionary-encode it again.
fn data_type(encoding: &Encoding) -> DataType {
    match encoding {
        Encoding::Int64 { .. } | Encoding::BoolRle { .. } => DataType::Int64,
        Encoding::Float64 { .. } => DataType::Float64,
        Encoding::Utf8 { .. } => DataType::Utf8,
        Encoding::Int64Dict { .. } => {
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Int64))
        }
        Encoding::Float64Dict { .. } => {
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Float64))
        }
        Encoding::Utf8Dict { .. } => {
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        }
    }
}

/// The engine's validity mask, as Arrow's null buffer.
///
/// A mask with no NULLs becomes **no** buffer at all: Arrow's writer treats
/// "no null buffer" and "a buffer of all ones" the same, and the smaller
/// array is the one the writer copies.
fn null_buffer(validity: &NullBitmap, len: usize) -> Option<NullBuffer> {
    if validity.null_count() == 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(validity.words().len() * 8);
    for word in validity.words() {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    Some(NullBuffer::new(BooleanBuffer::new(Buffer::from_vec(bytes), 0, len)))
}

/// One column chunk, as an Arrow array.
fn array_of(chunk: &ColumnChunk) -> Result<ArrayRef> {
    let len = chunk.len;
    let validity = &chunk.validity;
    let array: ArrayRef = match &chunk.encoding {
        Encoding::Int64 { values } => Arc::new(Int64Array::new(
            ScalarBuffer::from(values.clone()),
            null_buffer(validity, len),
        )) as ArrayRef,
        Encoding::Float64 { values } => Arc::new(Float64Array::new(
            ScalarBuffer::from(values.clone()),
            null_buffer(validity, len),
        )) as ArrayRef,
        Encoding::Utf8 { values } => string_array(values, validity, len),
        Encoding::Int64Dict { dict, indices } => {
            let values = Arc::new(Int64Array::from(dict.clone())) as ArrayRef;
            dictionary_array(indices.clone(), values, validity, len)?
        }
        Encoding::Float64Dict { dict, indices } => {
            let values = Arc::new(Float64Array::from(dict.clone())) as ArrayRef;
            dictionary_array(indices.clone(), values, validity, len)?
        }
        Encoding::Utf8Dict { dict, indices } => {
            let values = Arc::new(StringArray::from(dict.clone())) as ArrayRef;
            dictionary_array(indices.clone(), values, validity, len)?
        }
        Encoding::BoolRle { runs } => {
            // The engine's own decoder makes a run-length boolean into
            // integers, so the conversion does the same.
            let mut values: Vec<i64> = Vec::with_capacity(len);
            for &(value, count) in runs {
                values.extend(std::iter::repeat_n(i64::from(value), count as usize));
            }
            Arc::new(Int64Array::new(ScalarBuffer::from(values), null_buffer(validity, len)))
                as ArrayRef
        }
    };
    Ok(array)
}

/// The engine's strings, as Arrow's strings.
///
/// A NULL row's value is a placeholder in the engine's chunk -- the engine
/// keeps the slot, not the value -- so it is skipped here too, and Arrow's
/// offsets move by nothing for it. The written file does not depend on it: a
/// NULL row carries a definition level and no value.
fn string_array(values: &[String], validity: &NullBitmap, len: usize) -> ArrayRef {
    let mut offsets = Vec::with_capacity(len + 1);
    let mut bytes: Vec<u8> = Vec::new();
    offsets.push(0_i32);
    for (row, value) in values.iter().enumerate().take(len) {
        if validity.is_valid(row) {
            bytes.extend_from_slice(value.as_bytes());
        }
        offsets.push(i32::try_from(bytes.len()).expect("a string column fits in 32 bits"));
    }
    let offsets = arrow::buffer::OffsetBuffer::new(ScalarBuffer::from(offsets));
    Arc::new(StringArray::new(offsets, Buffer::from_vec(bytes), null_buffer(validity, len)))
}

/// The engine's dictionary, as Arrow's.
///
/// The keys keep the engine's indices and the values the engine's dictionary:
/// the Parquet writer interns the values again, per row group, so the
/// dictionary the engine built -- one for the whole file -- does not decide
/// the bytes.
fn dictionary_array(
    keys: Vec<i32>,
    values: ArrayRef,
    validity: &NullBitmap,
    len: usize,
) -> Result<ArrayRef> {
    let key_array = Int32Array::new(ScalarBuffer::from(keys), null_buffer(validity, len));
    DictionaryArray::<Int32Type>::try_new(key_array, values)
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| Error::Parquet(error.to_string()))
}
