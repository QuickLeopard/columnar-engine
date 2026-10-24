//! Batches.
//!
//! A batch is the unit of work in the engine: a fixed number of rows of one
//! or more columns, materialised into dense arrays so that the comparison
//! kernel can walk them without indirection.
//!
//! Batches are bounded at [`BATCH_SIZE`] rows. The size is not a magic
//! number -- it is chosen so that a batch of `f64` values fits comfortably in
//! L1/L2 and so that the selection vector for a batch is a small fixed
//! allocation. It is a named constant because the kernel, the operators and
//! the benchmarks all have to agree on it.

use crate::null::NullBitmap;

/// Rows per batch.
pub const BATCH_SIZE: usize = 2048;

/// The values of one column within a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchData {
    /// 64-bit integers.
    Int64(Vec<i64>),
    /// 64-bit doubles.
    Float64(Vec<f64>),
    /// UTF-8 strings.
    Utf8(Vec<String>),
}

impl BatchData {
    /// The physical type of the column.
    #[must_use]
    pub const fn data_type(&self) -> crate::types::DataType {
        match self {
            Self::Int64(_) => crate::types::DataType::Int64,
            Self::Float64(_) => crate::types::DataType::Float64,
            Self::Utf8(_) => crate::types::DataType::Utf8,
        }
    }

    /// Number of values held.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Int64(v) => v.len(),
            Self::Float64(v) => v.len(),
            Self::Utf8(v) => v.len(),
        }
    }

    /// True when the column holds no values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One column of a batch: its name, its values and its validity mask.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchColumn {
    /// The column's name, as it appears in the schema.
    pub name: String,
    /// Which rows of this column hold a value.
    pub validity: NullBitmap,
    /// The values, dense and aligned with `validity`.
    pub data: BatchData,
}

impl BatchColumn {
    /// Builds a fully valid column.
    #[must_use]
    pub fn new(name: impl Into<String>, data: BatchData) -> Self {
        let validity = NullBitmap::all_valid(data.len());
        Self { name: name.into(), validity, data }
    }

    /// Builds a column with an explicit validity mask.
    #[must_use]
    pub fn with_validity(name: impl Into<String>, validity: NullBitmap, data: BatchData) -> Self {
        debug_assert_eq!(validity.len(), data.len(), "validity mask and data length disagree");
        Self { name: name.into(), validity, data }
    }

    /// The physical type of the column.
    #[must_use]
    pub const fn data_type(&self) -> crate::types::DataType {
        self.data.data_type()
    }
}

/// A batch of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    /// Number of rows in this batch.
    pub len: usize,
    /// The columns, in schema order.
    pub columns: Vec<BatchColumn>,
}

impl Batch {
    /// Builds a batch from columns, taking the row count from the first.
    ///
    /// # Panics
    ///
    /// Panics if the columns disagree on their length.
    #[must_use]
    pub fn new(columns: Vec<BatchColumn>) -> Self {
        let len = columns.first().map_or(0, |c| c.data.len());
        for column in &columns {
            assert_eq!(
                column.data.len(),
                len,
                "column `{}` has {} rows, batch has {len}",
                column.name,
                column.data.len()
            );
        }
        Self { len, columns }
    }

    /// Number of rows in the batch.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the batch holds no rows.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Finds a column by name.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&BatchColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Finds a column index by name.
    #[must_use]
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}
