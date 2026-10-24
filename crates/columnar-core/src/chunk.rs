//! Column chunks: the engine's storage unit.
//!
//! A column chunk is one column's data for one contiguous range of rows,
//! stored in one of a small number of encodings. The engine decodes a chunk
//! into a dense [`crate::batch::BatchColumn`] before running operators over
//! it, so the *stored* representation and the *computed* representation are
//! deliberately different things.
//!
//! Two encodings are implemented because the review that hardened this design
//! was explicit that a columnar engine without dictionary-encoded strings is
//! a numeric toy: [`Encoding::Utf8Dict`] exists, and the TPC-H Q6 example
//! filters a dictionary-encoded string column, not a numeric one.
//!
//! ## Encodings
//!
//! | Encoding | Use |
//! |---|---|
//! | `Int64` / `Float64` / `Utf8` | plain, dense values |
//! | `Int64Dict` / `Float64Dict` / `Utf8Dict` | low-cardinality columns |
//! | `BoolRle` | boolean columns stored as runs |
//!
//! RLE over integers is deliberately absent: run-length encoding only pays for
//! itself when runs are long, and TPC-H's numeric columns do not have them.
//! Adding it would be a way to look comprehensive without a measurement
//! behind it.

use crate::batch::BatchColumn;
use crate::null::NullBitmap;
use crate::selection::SelectionVector;
use crate::types::{DataType, ScalarValue};

/// How a column chunk's values are stored.
#[derive(Debug, Clone, PartialEq)]
pub enum Encoding {
    /// Plain 64-bit integers.
    Int64 {
        /// The values, dense.
        values: Vec<i64>,
    },
    /// Plain 64-bit doubles.
    Float64 {
        /// The values, dense.
        values: Vec<f64>,
    },
    /// Plain UTF-8 strings.
    Utf8 {
        /// The values, dense.
        values: Vec<String>,
    },
    /// Dictionary-encoded integers.
    Int64Dict {
        /// The distinct values.
        dict: Vec<i64>,
        /// 32-bit indices into `dict`, one per row.
        indices: Vec<i32>,
    },
    /// Dictionary-encoded doubles.
    Float64Dict {
        /// The distinct values.
        dict: Vec<f64>,
        /// 32-bit indices into `dict`, one per row.
        indices: Vec<i32>,
    },
    /// Dictionary-encoded UTF-8 strings.
    Utf8Dict {
        /// The distinct values.
        dict: Vec<String>,
        /// 32-bit indices into `dict`, one per row.
        indices: Vec<i32>,
    },
    /// Run-length-encoded booleans.
    BoolRle {
        /// `(value, run length)` pairs, in row order.
        runs: Vec<(bool, u32)>,
    },
}

impl Encoding {
    /// A short, stable name used in the plan output and the dataset manifest.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Int64 { .. } => "int64",
            Self::Float64 { .. } => "float64",
            Self::Utf8 { .. } => "utf8",
            Self::Int64Dict { .. } => "int64_dict",
            Self::Float64Dict { .. } => "float64_dict",
            Self::Utf8Dict { .. } => "utf8_dict",
            Self::BoolRle { .. } => "bool_rle",
        }
    }

    /// The physical type of the encoded values.
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        match self {
            Self::Int64 { .. } | Self::Int64Dict { .. } | Self::BoolRle { .. } => DataType::Int64,
            Self::Float64 { .. } | Self::Float64Dict { .. } => DataType::Float64,
            Self::Utf8 { .. } | Self::Utf8Dict { .. } => DataType::Utf8,
        }
    }

    /// Number of rows encoded.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Int64 { values } => values.len(),
            Self::Float64 { values } => values.len(),
            Self::Utf8 { values } => values.len(),
            Self::Int64Dict { indices, .. }
            | Self::Float64Dict { indices, .. }
            | Self::Utf8Dict { indices, .. } => indices.len(),
            Self::BoolRle { runs } => runs.iter().map(|(_, n)| *n as usize).sum(),
        }
    }

    /// True when the encoding holds no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of distinct values, for dictionary encodings.
    #[must_use]
    pub fn dictionary_len(&self) -> Option<usize> {
        match self {
            Self::Int64Dict { dict, .. } => Some(dict.len()),
            Self::Float64Dict { dict, .. } => Some(dict.len()),
            Self::Utf8Dict { dict, .. } => Some(dict.len()),
            _ => None,
        }
    }
}

/// One column's data for one contiguous range of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnChunk {
    /// The column's name.
    pub name: String,
    /// The encoded values.
    pub encoding: Encoding,
    /// Which rows hold a value.
    pub validity: NullBitmap,
    /// Number of rows in the chunk.
    pub len: usize,
}

/// Reads a dictionary entry by its 32-bit index.
///
/// The index is signed because that is how a Parquet dictionary column
/// stores it. A negative index, or one past the end of the dictionary, is a
/// corrupt chunk rather than a value -- so it panics with a message that
/// says so, instead of wrapping or silently yielding the wrong element.
///
/// # Panics
///
/// Panics if `index` is negative or out of range for `dict`.
fn dict_get<T>(dict: &[T], index: i32) -> &T {
    let Ok(position) = usize::try_from(index) else {
        panic!("dictionary index {index} is negative");
    };
    dict.get(position).unwrap_or_else(|| {
        panic!("dictionary index {index} is out of range for a {} entry dictionary", dict.len())
    })
}

impl ColumnChunk {
    /// Builds a chunk, deriving the row count from the encoding and the
    /// validity mask.
    ///
    /// # Panics
    ///
    /// Panics if the encoding and the validity mask describe different
    /// numbers of rows.
    #[must_use]
    pub fn new(name: impl Into<String>, encoding: Encoding, validity: NullBitmap) -> Self {
        let name = name.into();
        let len = encoding.len();
        assert_eq!(
            validity.len(),
            len,
            "column `{}` has {len} encoded rows but a validity mask of {}",
            name,
            validity.len()
        );
        Self { name, encoding, validity, len }
    }

    /// The physical type of the column.
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        self.encoding.data_type()
    }

    /// Decodes the whole chunk into a dense column.
    #[must_use]
    pub fn decode(&self) -> BatchColumn {
        let data = match &self.encoding {
            Encoding::Int64 { values } => crate::batch::BatchData::Int64(values.clone()),
            Encoding::Float64 { values } => crate::batch::BatchData::Float64(values.clone()),
            Encoding::Utf8 { values } => crate::batch::BatchData::Utf8(values.clone()),
            Encoding::Int64Dict { dict, indices } => crate::batch::BatchData::Int64(
                indices.iter().map(|&i| *dict_get(dict, i)).collect(),
            ),
            Encoding::Float64Dict { dict, indices } => crate::batch::BatchData::Float64(
                indices.iter().map(|&i| *dict_get(dict, i)).collect(),
            ),
            Encoding::Utf8Dict { dict, indices } => crate::batch::BatchData::Utf8(
                indices.iter().map(|&i| dict_get(dict, i).clone()).collect(),
            ),
            Encoding::BoolRle { runs } => {
                let mut values = Vec::with_capacity(self.len);
                for &(value, count) in runs {
                    let value = i64::from(value);
                    values.extend(std::iter::repeat_n(value, count as usize));
                }
                crate::batch::BatchData::Int64(values)
            }
        };
        BatchColumn::with_validity(self.name.clone(), self.validity.clone(), data)
    }

    /// Decodes only the rows named by `selection`, in selection order.
    ///
    /// # Panics
    ///
    /// Panics if `selection` names a row outside this chunk.
    #[must_use]
    // The gather below indexes the value arrays directly. The index comes
    // from a selection vector over *this* chunk's rows, so it is in range by
    // construction; the `.get()` fallback would turn a logic bug into a
    // silent skip, which is worse than a panic.
    #[allow(clippy::indexing_slicing)]
    pub fn decode_gather(&self, selection: &SelectionVector) -> BatchColumn {
        let data = match &self.encoding {
            Encoding::Int64 { values } => {
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| out.push(values[row as usize]));
                crate::batch::BatchData::Int64(out)
            }
            Encoding::Float64 { values } => {
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| out.push(values[row as usize]));
                crate::batch::BatchData::Float64(out)
            }
            Encoding::Utf8 { values } => {
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| out.push(values[row as usize].clone()));
                crate::batch::BatchData::Utf8(out)
            }
            Encoding::Int64Dict { dict, indices } => {
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| out.push(*dict_get(dict, indices[row as usize])));
                crate::batch::BatchData::Int64(out)
            }
            Encoding::Float64Dict { dict, indices } => {
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| out.push(*dict_get(dict, indices[row as usize])));
                crate::batch::BatchData::Float64(out)
            }
            Encoding::Utf8Dict { dict, indices } => {
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| {
                    out.push(dict_get(dict, indices[row as usize]).clone());
                });
                crate::batch::BatchData::Utf8(out)
            }
            Encoding::BoolRle { runs } => {
                // RLE is decoded densely and then gathered, because a run
                // boundary lookup per selected row would be slower than the
                // linear scan it replaces for the row counts TPC-H produces.
                let mut dense = Vec::with_capacity(self.len);
                for &(value, count) in runs {
                    dense.extend(std::iter::repeat_n(i64::from(value), count as usize));
                }
                let mut out = Vec::with_capacity(selection.len());
                selection.for_each(|row| out.push(dense[row as usize]));
                crate::batch::BatchData::Int64(out)
            }
        };
        // The validity mask is gathered alongside the values. Reusing the
        // source mask here would be a bug: it describes the source row
        // numbers, not the gathered ones, so a selection that drops a NULL
        // row would still report that row as NULL, and one that keeps a NULL
        // row at a different position would report the wrong row as NULL.
        BatchColumn::with_validity(self.name.clone(), self.gather_validity(selection), data)
    }

    /// Gathers the validity mask for the rows named by `selection`.
    ///
    /// The gathered mask describes the *gathered* positions: the first
    /// selected row is position 0, the second is position 1, and so on. A
    /// selected row that was NULL in the source stays NULL.
    #[must_use]
    fn gather_validity(&self, selection: &SelectionVector) -> NullBitmap {
        if self.validity.null_count() == 0 {
            return NullBitmap::all_valid(selection.len());
        }
        let mut gathered = NullBitmap::all_valid(selection.len());
        let mut position = 0usize;
        selection.for_each(|row| {
            if !self.validity.is_valid(row as usize) {
                gathered.clear(position);
            }
            position += 1;
        });
        gathered
    }

    /// The value at `row`, as a scalar.
    ///
    /// Used by the aggregate operator for the small-cardinality columns that
    /// feed a group key, and by the test oracle.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range for this chunk.
    #[must_use]
    // As in `decode_gather`: the row index is checked against the chunk's
    // length at the top of the function, so the indexing below is in range
    // by construction.
    #[allow(clippy::indexing_slicing)]
    pub fn value_at(&self, row: usize) -> Option<ScalarValue> {
        assert!(row < self.len, "row {row} out of range for len {}", self.len);
        if !self.validity.is_valid(row) {
            return None;
        }
        Some(match &self.encoding {
            Encoding::Int64 { values } => ScalarValue::Int64(values[row]),
            Encoding::Float64 { values } => ScalarValue::Float64(values[row]),
            Encoding::Utf8 { values } => ScalarValue::Utf8(values[row].clone()),
            Encoding::Int64Dict { dict, indices } => {
                ScalarValue::Int64(*dict_get(dict, indices[row]))
            }
            Encoding::Float64Dict { dict, indices } => {
                ScalarValue::Float64(*dict_get(dict, indices[row]))
            }
            Encoding::Utf8Dict { dict, indices } => {
                ScalarValue::Utf8(dict_get(dict, indices[row]).clone())
            }
            Encoding::BoolRle { .. } => {
                let value = self.rle_value_at(row);
                ScalarValue::Int64(i64::from(value))
            }
        })
    }

    /// Finds the value of a run-length-encoded boolean at `row`.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range for the chunk.
    fn rle_value_at(&self, row: usize) -> bool {
        assert!(row < self.len, "row {row} out of range for len {}", self.len);
        let Encoding::BoolRle { runs } = &self.encoding else {
            unreachable!("rle_value_at called on a non-RLE encoding");
        };
        let mut offset = 0usize;
        for &(value, count) in runs {
            offset += count as usize;
            if offset > row {
                return value;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{ColumnChunk, Encoding};
    use crate::null::NullBitmap;
    use crate::selection::SelectionVector;

    /// A gathered column must describe the *gathered* rows, not the source
    /// rows. Reusing the source validity mask is a bug that only shows up
    /// when the selection both drops and keeps NULL rows.
    #[test]
    fn gather_carries_the_validity_mask_with_the_values() {
        let chunk = ColumnChunk::new(
            "l_discount",
            Encoding::Float64 { values: vec![1.0, 2.0, 3.0, 4.0, 5.0] },
            // Rows 1 and 3 are NULL.
            NullBitmap::with_nulls(5, &[1, 3]),
        );

        // Keep rows 0, 1 and 3: row 1 and row 3 are NULL, and they land at
        // gathered positions 1 and 2.
        let selection = SelectionVector::from_rows(vec![0, 1, 3], 5);
        let gathered = chunk.decode_gather(&selection);

        assert_eq!(gathered.data.len(), 3);
        assert_eq!(gathered.validity.len(), 3);
        assert_eq!(gathered.validity.null_count(), 2);
        assert!(gathered.validity.is_valid(0));
        assert!(!gathered.validity.is_valid(1));
        assert!(!gathered.validity.is_valid(2));
    }

    /// Gathering only the valid rows must produce a column with no NULLs at
    /// all -- not a column whose mask still says the source rows were NULL.
    #[test]
    fn gathering_only_valid_rows_leaves_no_nulls() {
        let chunk = ColumnChunk::new(
            "l_discount",
            Encoding::Float64 { values: vec![1.0, 2.0, 3.0, 4.0, 5.0] },
            NullBitmap::with_nulls(5, &[1, 3]),
        );

        let selection = SelectionVector::from_rows(vec![0, 2, 4], 5);
        let gathered = chunk.decode_gather(&selection);

        assert_eq!(gathered.validity.null_count(), 0);
        assert!(gathered.validity.is_valid(0));
        assert!(gathered.validity.is_valid(1));
        assert!(gathered.validity.is_valid(2));
    }

    /// A dictionary-encoded column gathers its values and its mask together.
    #[test]
    fn gather_works_for_dictionary_encoded_columns() {
        let chunk = ColumnChunk::new(
            "l_shipdate",
            Encoding::Utf8Dict {
                dict: vec!["1994-01-01".to_owned(), "1995-01-01".to_owned()],
                indices: vec![0, 1, 0, 1],
            },
            NullBitmap::with_nulls(4, &[2]),
        );

        let selection = SelectionVector::from_rows(vec![1, 2, 3], 4);
        let gathered = chunk.decode_gather(&selection);

        assert_eq!(gathered.validity.len(), 3);
        assert_eq!(gathered.validity.null_count(), 1);
        assert!(!gathered.validity.is_valid(1));
        match &gathered.data {
            crate::batch::BatchData::Utf8(values) => {
                assert_eq!(values, &["1995-01-01", "1994-01-01", "1995-01-01"]);
            }
            other => panic!("expected a utf8 column, got {other:?}"),
        }
    }
}
