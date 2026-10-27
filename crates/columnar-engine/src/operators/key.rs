//! The keys a join and a sort read.
//!
//! Both operators need one thing the filter does not: a column's value, one
//! row at a time, at an arbitrary row. The filter always walks a column in
//! order, so it can work on the encoding directly; a join probes a row the
//! hash table points at, and a sort compares rows that are nowhere near each
//! other.
//!
//! The values are read **where they lie**. A column nobody asked to be
//! materialised is never copied, so a join's key costs a lookup rather than
//! a copy of the column -- and a dictionary-encoded column is read through
//! its dictionary, exactly as the gather would have copied it out of it.

use columnar_core::{BatchColumn, BatchData, ColumnChunk, Encoding, NullBitmap};

/// A value a join or a sort can key on.
///
/// The value borrows from the column it was read out of, so reading a key
/// costs a lookup rather than a copy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyValue<'a> {
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit double.
    Float(f64),
    /// A string.
    Text(&'a str),
}

/// A column read one row at a time, where it lies.
#[derive(Debug, Clone)]
pub struct KeyColumn<'a> {
    /// Where the values are.
    values: KeyValues<'a>,
    /// Which rows hold a value.
    validity: &'a NullBitmap,
}

/// Where a key column's values are.
#[derive(Debug, Clone)]
enum KeyValues<'a> {
    /// Plain integers.
    Int64(&'a [i64]),
    /// Plain doubles.
    Float64(&'a [f64]),
    /// Plain strings.
    Utf8(&'a [String]),
    /// Dictionary-encoded integers.
    Int64Dict {
        /// The distinct values.
        dict: &'a [i64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded doubles.
    Float64Dict {
        /// The distinct values.
        dict: &'a [f64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded strings.
    Utf8Dict {
        /// The distinct values.
        dict: &'a [String],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// A decoded column.
    ///
    /// A run-length-encoded column holds runs rather than values, so reading
    /// one of its rows means finding the run the row falls in. The column is
    /// decoded once instead, and the rows are read out of the copy.
    Decoded(BatchData),
}

impl<'a> KeyColumn<'a> {
    /// The column in `chunk`.
    pub fn from_chunk(chunk: &'a ColumnChunk) -> Self {
        let values = match &chunk.encoding {
            Encoding::Int64 { values } => KeyValues::Int64(values),
            Encoding::Float64 { values } => KeyValues::Float64(values),
            Encoding::Utf8 { values } => KeyValues::Utf8(values),
            Encoding::Int64Dict { dict, indices } => KeyValues::Int64Dict { dict, indices },
            Encoding::Float64Dict { dict, indices } => KeyValues::Float64Dict { dict, indices },
            Encoding::Utf8Dict { dict, indices } => KeyValues::Utf8Dict { dict, indices },
            Encoding::BoolRle { .. } => KeyValues::Decoded(chunk.decode().data),
        };
        Self { values, validity: &chunk.validity }
    }

    /// The column in an already-decoded column.
    pub fn from_decoded(column: &'a BatchColumn) -> Self {
        let values = match &column.data {
            BatchData::Int64(values) => KeyValues::Int64(values),
            BatchData::Float64(values) => KeyValues::Float64(values),
            BatchData::Utf8(values) => KeyValues::Utf8(values),
        };
        Self { values, validity: &column.validity }
    }

    /// The value of `row`, if it holds one.
    ///
    /// A row that holds no value has no key: a NULL never matches anything,
    /// and it has no place in an order either.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range for the column. The rows come from a
    /// selection built over this very column, so they are in range by
    /// construction.
    pub fn value_at(&self, row: usize) -> Option<KeyValue<'_>> {
        if row >= self.validity.len() || !self.validity.is_valid(row) {
            return None;
        }
        Some(match &self.values {
            KeyValues::Int64(values) => KeyValue::Int(values[row]),
            KeyValues::Float64(values) => KeyValue::Float(values[row]),
            KeyValues::Utf8(values) => KeyValue::Text(values[row].as_str()),
            KeyValues::Int64Dict { dict, indices } => KeyValue::Int(dict[dictionary(indices, row)]),
            KeyValues::Float64Dict { dict, indices } => {
                KeyValue::Float(dict[dictionary(indices, row)])
            }
            KeyValues::Utf8Dict { dict, indices } => {
                KeyValue::Text(dict[dictionary(indices, row)].as_str())
            }
            KeyValues::Decoded(data) => match data {
                BatchData::Int64(values) => KeyValue::Int(values[row]),
                BatchData::Float64(values) => KeyValue::Float(values[row]),
                BatchData::Utf8(values) => KeyValue::Text(values[row].as_str()),
            },
        })
    }
}

/// The dictionary index of `row`.
///
/// # Panics
///
/// Panics if the index is negative, which a dictionary index never is.
fn dictionary(indices: &[i32], row: usize) -> usize {
    let index = indices[row];
    usize::try_from(index).expect("a dictionary index is not negative")
}
