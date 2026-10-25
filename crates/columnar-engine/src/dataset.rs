//! Parquet ingestion.
//!
//! A [`Dataset`] is a set of [`ColumnChunk`]s, one per column, in schema
//! order. The reader is the only part of the engine that knows Parquet
//! exists: above this module, a column is a chunk with an encoding and a
//! validity mask, and nothing else.
//!
//! ## The layouts the reader accepts
//!
//! | Arrow array | Engine encoding |
//! |---|---|
//! | `Int64Array` | [`Encoding::Int64`] |
//! | `Float64Array` | [`Encoding::Float64`] |
//! | `StringArray` | [`Encoding::Utf8`] |
//! | `DictionaryArray<Int32Type>` over UTF-8 | [`Encoding::Utf8Dict`] |
//!
//! A dictionary-encoded string column keeps its dictionary: the dictionary
//! holds the distinct values and the column holds 32-bit indices into it.
//! That is the layout the TPC-H `lineitem` string columns arrive in.
//!
//! A column that arrives as a plain string array is stored plainly. Both
//! are accepted, because the reader must not depend on the writer having
//! dictionary-encoded a column.
//!
//! A row that holds NULL still has a slot in the value array, because the
//! engine's value arrays are dense; the validity mask says which slots hold
//! a value. The slot of a NULL row holds a zero, and the mask says it means
//! nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use arrow::array::{
    Array, DictionaryArray, Float64Array, Int64Array, LargeStringArray, StringArray,
};
use arrow::datatypes::{DataType as ArrowType, Int32Type, SchemaRef};
use columnar_core::{ColumnChunk, DataType, Encoding, NullBitmap};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
/// Everything that can go wrong while reading a dataset.
#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    /// The file could not be opened or read.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a Parquet file this reader understands.
    #[error("parquet error: {0}")]
    Parquet(String),
    /// The file holds a column type the engine does not support.
    #[error("unsupported column type: {0}")]
    UnsupportedType(String),
}

/// A set of columns, in schema order.
///
/// The row count is stored rather than derived, because a scan whose
/// predicates were all pushed into the Parquet reader can produce **rows and
/// no columns**: a `count(*)` over a fully pushed-down filter needs no
/// column at all, and the reader returns zero-column batches that still
/// carry the surviving row count. A dataset that holds a column always has
/// that column's length as its row count.
#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    /// The columns, in schema order.
    pub chunks: Vec<ColumnChunk>,
    /// The number of rows in the dataset.
    pub rows: usize,
}

impl Dataset {
    /// A dataset over the given columns.
    ///
    /// The row count is the first column's length, or zero when there are no
    /// columns.
    #[must_use]
    pub fn from_chunks(chunks: Vec<ColumnChunk>) -> Self {
        let rows = chunks.first().map_or(0, |chunk| chunk.len);
        Self { chunks, rows }
    }

    /// A dataset over the given columns and row count.
    ///
    /// The row count is only meaningful when there are no columns: with a
    /// column present, the column's length is the row count, and a
    /// disagreement would mean the data and the count contradict each other.
    #[must_use]
    pub fn with_rows(chunks: Vec<ColumnChunk>, rows: usize) -> Self {
        Self { chunks, rows }
    }

    /// Reads a Parquet file into column chunks.
    ///
    /// The file may hold several row groups and several record batches; the
    /// chunks that come back cover every row of the file, in schema order.
    ///
    /// This is the **eager** read: every column of the file is decoded,
    /// whether the query needs it or not. The lazy read, which decodes only
    /// what a plan needs, is [`crate::source::ParquetSource`].
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Io`] if the file cannot be opened, and
    /// [`DatasetError::Parquet`] if it is not a Parquet file the reader
    /// understands.
    pub fn read_parquet(path: &Path) -> Result<Self, DatasetError> {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|error| DatasetError::Parquet(error.to_string()))?;
        let schema = builder.schema().clone();
        let indices: Vec<usize> = (0..schema.fields().len()).collect();

        let reader = builder.build().map_err(|error| DatasetError::Parquet(error.to_string()))?;
        decode_batches(reader, &schema, &indices)
    }

    /// Number of rows in the dataset.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows
    }

    /// True when the dataset has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Finds a chunk by column name.
    #[must_use]
    pub fn chunk(&self, name: &str) -> Option<&ColumnChunk> {
        self.chunks.iter().find(|chunk| chunk.name == name)
    }

    /// The column names, in schema order.
    #[must_use]
    pub fn column_names(&self) -> Vec<String> {
        self.chunks.iter().map(|chunk| chunk.name.clone()).collect()
    }
}

impl crate::plan::ColumnSchema for Dataset {
    fn column_count(&self) -> usize {
        self.chunks.len()
    }

    fn column_name(&self, index: usize) -> &str {
        self.chunks.get(index).map_or("", |chunk| chunk.name.as_str())
    }

    fn column_type(&self, index: usize) -> DataType {
        self.chunks.get(index).map_or(DataType::Int64, columnar_core::ColumnChunk::data_type)
    }
}

/// Decodes record batches into column chunks.
///
/// `indices` names the file's root columns the batches hold, in ascending
/// order. The batches' columns are in that same order, because the reader
/// hands them over in schema order.
///
/// The row count is taken from the batches rather than from the file's
/// metadata: a scan with pushed-down predicates returns fewer rows than the
/// file holds, and it is the surviving rows that were decoded.
pub(crate) fn decode_batches(
    reader: impl Iterator<Item = Result<arrow::record_batch::RecordBatch, arrow::error::ArrowError>>,
    schema: &SchemaRef,
    indices: &[usize],
) -> Result<Dataset, DatasetError> {
    let mut builders = builders_for(schema, indices)?;
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.map_err(|error| DatasetError::Parquet(error.to_string()))?;
        for (position, column) in batch.columns().iter().enumerate() {
            let Some(builder) = builders.get_mut(position) else {
                return Err(DatasetError::Parquet(
                    "the record batch holds more columns than were projected".to_owned(),
                ));
            };
            builder.push(column.as_ref(), rows)?;
        }
        rows += batch.num_rows();
    }

    let mut chunks = Vec::with_capacity(builders.len());
    for builder in builders {
        chunks.push(builder.finish(rows)?);
    }
    Ok(Dataset::with_rows(chunks, rows))
}

/// Accumulates one column across the record batches of a file.
enum ColumnBuilder {
    /// A column of 64-bit integers.
    Int64 {
        /// The column's name.
        name: String,
        /// The values, dense.
        values: Vec<i64>,
        /// The rows that hold no value.
        nulls: Vec<usize>,
    },
    /// A column of 64-bit doubles.
    Float64 {
        /// The column's name.
        name: String,
        /// The values, dense.
        values: Vec<f64>,
        /// The rows that hold no value.
        nulls: Vec<usize>,
    },
    /// A column of strings.
    Utf8 {
        /// The column's name.
        name: String,
        /// The values, dense.
        values: Vec<String>,
        /// The rows that hold no value.
        nulls: Vec<usize>,
    },
    /// A dictionary-encoded column of strings.
    Utf8Dict {
        /// The column's name.
        name: String,
        /// The distinct values.
        dict: Vec<String>,
        /// The index of each distinct value in `dict`.
        lookup: HashMap<String, usize>,
        /// The 32-bit index of each row into `dict`.
        indices: Vec<i32>,
        /// The rows that hold no value.
        nulls: Vec<usize>,
    },
}

impl ColumnBuilder {
    /// The builder for a column of the given name and Arrow type.
    fn new(name: &str, data_type: &ArrowType) -> Result<Self, DatasetError> {
        match data_type {
            ArrowType::Int64 => {
                Ok(Self::Int64 { name: name.to_owned(), values: Vec::new(), nulls: Vec::new() })
            }
            ArrowType::Float64 => {
                Ok(Self::Float64 { name: name.to_owned(), values: Vec::new(), nulls: Vec::new() })
            }
            ArrowType::Utf8 | ArrowType::LargeUtf8 => {
                Ok(Self::Utf8 { name: name.to_owned(), values: Vec::new(), nulls: Vec::new() })
            }
            ArrowType::Dictionary(_, value) => match value.as_ref() {
                ArrowType::Utf8 | ArrowType::LargeUtf8 => Ok(Self::Utf8Dict {
                    name: name.to_owned(),
                    dict: Vec::new(),
                    lookup: HashMap::new(),
                    indices: Vec::new(),
                    nulls: Vec::new(),
                }),
                other => Err(DatasetError::UnsupportedType(format!(
                    "a dictionary column over {other} is not supported"
                ))),
            },
            other => Err(DatasetError::UnsupportedType(format!("{other}"))),
        }
    }

    /// Adds one record batch's worth of values.
    fn push(&mut self, array: &dyn Array, row_offset: usize) -> Result<(), DatasetError> {
        match self {
            Self::Int64 { name, values, nulls } => {
                let array = downcast::<Int64Array>(array, &format!("int64 column `{name}`"))?;
                let batch_nulls = null_rows(array, row_offset);
                let offset = values.len();
                values.extend_from_slice(array.values());
                for &row in &batch_nulls {
                    // A null row's value is undefined. The engine's
                    // convention is zero, and the validity bitmap is what
                    // says the row is NULL.
                    values[offset + (row - row_offset)] = 0;
                }
                nulls.extend(batch_nulls);
            }
            Self::Float64 { name, values, nulls } => {
                let array = downcast::<Float64Array>(array, &format!("float64 column `{name}`"))?;
                let batch_nulls = null_rows(array, row_offset);
                let offset = values.len();
                values.extend_from_slice(array.values());
                for &row in &batch_nulls {
                    values[offset + (row - row_offset)] = 0.0;
                }
                nulls.extend(batch_nulls);
            }
            Self::Utf8 { name, values, nulls } => {
                values.extend(string_values(array, name)?);
                nulls.extend(null_rows(array, row_offset));
            }
            Self::Utf8Dict { name, dict, lookup, indices, nulls } => {
                let array = downcast::<DictionaryArray<Int32Type>>(
                    array,
                    &format!("dictionary column `{name}`"),
                )?;
                let strings = dictionary_values(array.values().as_ref(), name)?;

                // Merge the batch's dictionary into the column's dictionary,
                // so one dictionary covers the whole file. The values are
                // borrowed: a value the column already holds is not copied,
                // so this allocates once per *distinct* value over the whole
                // file rather than once per batch.
                let mut remap = Vec::with_capacity(strings.len());
                for value in &strings {
                    let slot = if let Some(&slot) = lookup.get(*value) {
                        slot
                    } else {
                        let slot = dict.len();
                        dict.push((*value).to_owned());
                        lookup.insert((*value).to_owned(), slot);
                        slot
                    };
                    remap.push(slot);
                }

                let keys = array.keys();
                let has_nulls = keys.null_count() > 0;
                for row in 0..keys.len() {
                    if has_nulls && keys.is_null(row) {
                        // A null key is a NULL value.
                        indices.push(0);
                        nulls.push(row_offset + row);
                        continue;
                    }
                    let key = keys.value(row);
                    let slot = usize::try_from(key).ok().and_then(|key| remap.get(key)).copied();
                    let Some(slot) = slot else {
                        return Err(DatasetError::Parquet(format!(
                            "a dictionary key {key} is outside the dictionary"
                        )));
                    };
                    indices.push(i32::try_from(slot).map_err(|_| {
                        DatasetError::UnsupportedType(
                            "a dictionary holds more than i32::MAX values".to_owned(),
                        )
                    })?);
                }
            }
        }
        Ok(())
    }

    /// The finished chunk.
    fn finish(self, rows: usize) -> Result<ColumnChunk, DatasetError> {
        let validity = NullBitmap::with_nulls(rows, self.nulls());
        let (name, encoding) = match self {
            Self::Int64 { name, values, .. } => (name, Encoding::Int64 { values }),
            Self::Float64 { name, values, .. } => (name, Encoding::Float64 { values }),
            Self::Utf8 { name, values, .. } => (name, Encoding::Utf8 { values }),
            Self::Utf8Dict { name, dict, indices, .. } => {
                (name, Encoding::Utf8Dict { dict, indices })
            }
        };
        if encoding.len() != rows {
            return Err(DatasetError::Parquet(format!(
                "column `{name}` holds {} rows but the file holds {rows}",
                encoding.len()
            )));
        }
        Ok(ColumnChunk::new(name, encoding, validity))
    }

    /// The rows that hold no value.
    fn nulls(&self) -> &[usize] {
        match self {
            Self::Int64 { nulls, .. }
            | Self::Float64 { nulls, .. }
            | Self::Utf8 { nulls, .. }
            | Self::Utf8Dict { nulls, .. } => nulls,
        }
    }
}

/// The string values of an Arrow array.
fn string_values(array: &dyn Array, name: &str) -> Result<Vec<String>, DatasetError> {
    if let Some(array) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(array
            .iter()
            .map(|value| value.map_or_else(String::new, ToOwned::to_owned))
            .collect());
    }
    if let Some(array) = array.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(array
            .iter()
            .map(|value| value.map_or_else(String::new, ToOwned::to_owned))
            .collect());
    }
    Err(DatasetError::UnsupportedType(format!("column `{name}` is not a utf8 column")))
}

/// The distinct values of a dictionary, **borrowed**.
///
/// The dictionary path needs the distinct values only to merge one batch's
/// dictionary into the column's, and the merge is a lookup: the column's
/// dictionary already holds the string, so nothing needs to be copied. This
/// function therefore borrows.
///
/// The difference is not academic. A `lineitem` file has 2,500 distinct
/// ship dates and the reader hands the file over in batches, so an owning
/// version of this would allocate and copy 2,500 strings *per batch* --
/// 1.8 million string allocations for the 733 batches of a six-million-row
/// file -- to throw all but the first batch's worth away.
fn dictionary_values<'a>(array: &'a dyn Array, name: &str) -> Result<Vec<&'a str>, DatasetError> {
    if let Some(array) = array.as_any().downcast_ref::<StringArray>() {
        return Ok(array.iter().map(Option::unwrap_or_default).collect());
    }
    if let Some(array) = array.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(array.iter().map(Option::unwrap_or_default).collect());
    }
    Err(DatasetError::UnsupportedType(format!("column `{name}` is not a utf8 dictionary column")))
}

/// Downcasts an Arrow array, or reports that the file holds another type.
fn downcast<'a, T: 'static>(array: &'a dyn Array, what: &str) -> Result<&'a T, DatasetError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| DatasetError::UnsupportedType(format!("expected a {what}")))
}

/// The rows of `array` that hold no value, offset by `row_offset`.
fn null_rows(array: &dyn Array, row_offset: usize) -> Vec<usize> {
    if array.null_count() == 0 {
        return Vec::new();
    }
    (0..array.len()).filter(|&row| array.is_null(row)).map(|row| row_offset + row).collect()
}

/// One builder per projected column of the schema.
fn builders_for(schema: &SchemaRef, indices: &[usize]) -> Result<Vec<ColumnBuilder>, DatasetError> {
    indices
        .iter()
        .map(|&index| {
            let field = schema.fields().get(index).ok_or_else(|| {
                DatasetError::Parquet(format!("the schema has no column at index {index}"))
            })?;
            ColumnBuilder::new(field.name(), field.data_type())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The decode counter
//
// Late materialisation is only worth anything if it can be checked, so the
// engine counts the columns it decodes. The counter is per thread and off by
// default: a thread that has not asked for the counter pays one thread-local
// lookup per decoded column and nothing else. The tests turn it on.
// ---------------------------------------------------------------------------

thread_local! {
    /// The columns this thread decoded, and how often. `None` means the
    /// counter is off.
    static DECODE_COUNTS: RefCell<Option<HashMap<String, usize>>> = const { RefCell::new(None) };
}

/// Records that the engine touched a column.
pub(crate) fn note_decode(name: &str) {
    DECODE_COUNTS.with(|counts| {
        let mut counts = counts.borrow_mut();
        if let Some(counts) = counts.as_mut() {
            *counts.entry(name.to_owned()).or_insert(0) += 1;
        }
    });
}

/// Test-only hook: starts counting the columns this thread decodes.
///
/// The counter is off by default; a test turns it on, runs a plan, and then
/// reads [`decode_counts`].
#[doc(hidden)]
pub fn enable_decode_counting() {
    DECODE_COUNTS.with(|counts| {
        *counts.borrow_mut() = Some(HashMap::new());
    });
}

/// Test-only hook: stops counting the columns this thread decodes.
#[doc(hidden)]
pub fn disable_decode_counting() {
    DECODE_COUNTS.with(|counts| {
        *counts.borrow_mut() = None;
    });
}

/// Test-only hook: the columns this thread decoded, and how often.
///
/// Only the columns that were decoded appear; a column the plan never
/// touched is absent, which is the whole point of the hook.
#[doc(hidden)]
#[must_use]
pub fn decode_counts() -> HashMap<String, usize> {
    DECODE_COUNTS.with(|counts| counts.borrow().clone().unwrap_or_default())
}

/// Merges several datasets, in order, into one.
///
/// This is what a parallel read needs: each thread produces a partial dataset
/// over its own row groups, and the parts come back together in the file's
/// row-group order.
///
/// **The order is not negotiable.** `SUM` is not associative, so a dataset
/// whose rows were ordered by *completion* rather than by *position* would
/// make the engine's answer depend on the scheduler, and the same query would
/// return different bits on different runs. The merged dataset therefore
/// holds the rows in the order the parts were given, and a parallel read
/// gives its parts in row-group order.
///
/// # Errors
///
/// Returns [`DatasetError::Parquet`] if two parts disagree about a column's
/// encoding, or if a part holds an encoding this merge does not know how to
/// combine.
pub fn merge(parts: &[Dataset]) -> Result<Dataset, DatasetError> {
    let Some(first) = parts.first() else {
        return Ok(Dataset::with_rows(Vec::new(), 0));
    };
    if parts.len() == 1 {
        return Ok(first.clone());
    }

    let column_count = first.chunks.len();
    let mut chunks = Vec::with_capacity(column_count);
    for column in 0..column_count {
        let mut merged: Option<ColumnChunk> = None;
        for part in parts {
            let chunk = part.chunks.get(column).ok_or_else(|| {
                DatasetError::Parquet(format!(
                    "a part of the merge has {} columns where the first has {column_count}",
                    part.chunks.len()
                ))
            })?;
            merged = Some(match merged {
                None => chunk.clone(),
                Some(previous) => merge_chunk(&previous, chunk)?,
            });
        }
        chunks.push(merged.unwrap_or_else(|| first.chunks[column].clone()));
    }

    let rows = parts.iter().map(Dataset::len).sum();
    Ok(Dataset::with_rows(chunks, rows))
}

/// Merges two chunks of the same column.
fn merge_chunk(left: &ColumnChunk, right: &ColumnChunk) -> Result<ColumnChunk, DatasetError> {
    let name = left.name.clone();
    let validity = left.validity.concat(&right.validity);
    let encoding = match (&left.encoding, &right.encoding) {
        (Encoding::Int64 { values: left_values }, Encoding::Int64 { values: right_values }) => {
            let mut values = left_values.clone();
            values.extend_from_slice(right_values);
            Encoding::Int64 { values }
        }
        (Encoding::Float64 { values: left_values }, Encoding::Float64 { values: right_values }) => {
            let mut values = left_values.clone();
            values.extend_from_slice(right_values);
            Encoding::Float64 { values }
        }
        (Encoding::Utf8 { values: left_values }, Encoding::Utf8 { values: right_values }) => {
            let mut values = left_values.clone();
            values.extend(right_values.iter().cloned());
            Encoding::Utf8 { values }
        }
        (
            Encoding::Utf8Dict { dict: left_dict, indices: left_indices },
            Encoding::Utf8Dict { dict: right_dict, indices: right_indices },
        ) => {
            let (dict, indices) =
                merge_dictionary(left_dict, left_indices, right_dict, right_indices);
            Encoding::Utf8Dict { dict, indices }
        }
        (left, right) => {
            return Err(DatasetError::Parquet(format!(
                "cannot merge a `{}` column with a `{}` one",
                left.name(),
                right.name()
            )))
        }
    };
    Ok(ColumnChunk::new(name, encoding, validity))
}

/// Merges two dictionary-encoded string columns.
///
/// The two dictionaries may hold different distinct values, so the second
/// one's entries are looked up in the first one's and remapped. A value the
/// first dictionary does not hold is added to it, once, and every row that
/// used it is repointed.
///
/// This is the same operation the engine performs per batch when it merges a
/// file's row groups into one dictionary. Doing it once, at the end, is the
/// same work with the same answer.
fn merge_dictionary(
    left_dict: &[String],
    left_indices: &[i32],
    right_dict: &[String],
    right_indices: &[i32],
) -> (Vec<String>, Vec<i32>) {
    let mut dict = left_dict.to_vec();
    // The lookup owns its keys, because the dictionary it points into grows
    // as values are added.
    let mut lookup: HashMap<String, usize> = HashMap::with_capacity(dict.len());
    for (index, value) in dict.iter().enumerate() {
        lookup.insert(value.clone(), index);
    }

    let mut indices = left_indices.to_vec();
    indices.reserve(right_indices.len());
    for &key in right_indices {
        // A negative or out-of-range index is a corrupt chunk. It cannot
        // happen from this engine's own reader, and silently mapping it to
        // some other value would turn a corrupt file into a wrong answer.
        let Ok(position) = usize::try_from(key) else {
            panic!("dictionary index {key} is negative");
        };
        let Some(value) = right_dict.get(position) else {
            panic!(
                "dictionary index {key} is out of range for a {} entry dictionary",
                right_dict.len()
            );
        };
        let slot = if let Some(&slot) = lookup.get(value.as_str()) {
            slot
        } else {
            let slot = dict.len();
            lookup.insert(value.clone(), slot);
            dict.push(value.clone());
            slot
        };
        indices.push(i32::try_from(slot).expect("a dictionary slot fits in 32 bits"));
    }
    (dict, indices)
}
