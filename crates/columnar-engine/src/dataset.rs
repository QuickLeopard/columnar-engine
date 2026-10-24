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
use arrow::datatypes::{DataType, Int32Type, SchemaRef};
use columnar_core::{ColumnChunk, Encoding, NullBitmap};
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
#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    /// The columns, in schema order.
    pub chunks: Vec<ColumnChunk>,
}

impl Dataset {
    /// Reads a Parquet file into column chunks.
    ///
    /// The file may hold several row groups and several record batches; the
    /// chunks that come back cover every row of the file, in schema order.
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
        let mut builders = builders_for(&schema)?;

        let reader = builder.build().map_err(|error| DatasetError::Parquet(error.to_string()))?;
        let mut rows = 0usize;
        for batch in reader {
            let batch = batch.map_err(|error| DatasetError::Parquet(error.to_string()))?;
            for (index, column) in batch.columns().iter().enumerate() {
                let Some(builder) = builders.get_mut(index) else {
                    return Err(DatasetError::Parquet(
                        "the record batch holds more columns than the schema".to_owned(),
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
        Ok(Self { chunks })
    }

    /// Number of rows in the dataset.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.first().map_or(0, |chunk| chunk.len)
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
    fn new(name: &str, data_type: &DataType) -> Result<Self, DatasetError> {
        match data_type {
            DataType::Int64 => {
                Ok(Self::Int64 { name: name.to_owned(), values: Vec::new(), nulls: Vec::new() })
            }
            DataType::Float64 => {
                Ok(Self::Float64 { name: name.to_owned(), values: Vec::new(), nulls: Vec::new() })
            }
            DataType::Utf8 | DataType::LargeUtf8 => {
                Ok(Self::Utf8 { name: name.to_owned(), values: Vec::new(), nulls: Vec::new() })
            }
            DataType::Dictionary(_, value) => match value.as_ref() {
                DataType::Utf8 | DataType::LargeUtf8 => Ok(Self::Utf8Dict {
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
                values.extend(array.iter().map(Option::unwrap_or_default));
                nulls.extend(null_rows(array, row_offset));
            }
            Self::Float64 { name, values, nulls } => {
                let array = downcast::<Float64Array>(array, &format!("float64 column `{name}`"))?;
                values.extend(array.iter().map(Option::unwrap_or_default));
                nulls.extend(null_rows(array, row_offset));
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
                let strings = string_values(array.values().as_ref(), name)?;

                // Merge the batch's dictionary into the column's dictionary,
                // so one dictionary covers the whole file.
                let mut remap = Vec::with_capacity(strings.len());
                for value in &strings {
                    let slot = if let Some(&slot) = lookup.get(value) {
                        slot
                    } else {
                        let slot = dict.len();
                        dict.push(value.clone());
                        lookup.insert(value.clone(), slot);
                        slot
                    };
                    remap.push(slot);
                }

                let keys = array.keys();
                for row in 0..keys.len() {
                    if keys.is_null(row) {
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

/// One builder per column of the schema.
fn builders_for(schema: &SchemaRef) -> Result<Vec<ColumnBuilder>, DatasetError> {
    schema
        .fields()
        .iter()
        .map(|field| ColumnBuilder::new(field.name(), field.data_type()))
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
