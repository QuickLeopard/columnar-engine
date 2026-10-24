//! The projection operator, and late materialisation with it.
//!
//! Late materialisation is the rule that a column is decoded only when the
//! plan actually reads it. A column that is neither filtered nor aggregated
//! is never touched, and the plan output names the columns that were, so the
//! claim is checkable rather than asserted.
//!
//! ## Gathering
//!
//! [`gather`] reads the rows a [`SelectionVector`] names out of a
//! [`ColumnChunk`], and gathers the validity mask with them: the returned
//! column has one validity bit per *gathered* row, in selection order.
//!
//! The core's own `ColumnChunk::decode_gather` cannot be used for a partial
//! selection: it returns the *source* validity mask next to the *gathered*
//! values, so the mask and the data disagree on length, which trips the
//! `debug_assert` inside `BatchColumn::with_validity`. The gather below
//! mirrors the core decoder's rules and gathers the mask with the values, so
//! it is correct for every selection; it can be deleted in favour of the
//! core decoder once the core decoder learns to gather the mask.

use std::collections::HashMap;

use columnar_core::{BatchColumn, BatchData, ColumnChunk, Encoding, NullBitmap, SelectionVector};

use crate::dataset::note_decode;
use crate::Dataset;

/// Gathers the rows named by `selection` out of `chunk`.
///
/// The validity mask is gathered with the values.
#[must_use]
pub fn gather(chunk: &ColumnChunk, selection: &SelectionVector) -> BatchColumn {
    note_decode(&chunk.name);
    let validity = gather_validity(&chunk.validity, selection);
    let data = gather_encoding(&chunk.encoding, selection);
    BatchColumn::with_validity(chunk.name.clone(), validity, data)
}

/// Gathers the rows named by `selection` out of an already-dense column.
#[must_use]
pub fn gather_dense(column: &BatchColumn, selection: &SelectionVector) -> BatchColumn {
    let validity = gather_validity(&column.validity, selection);
    let data = match &column.data {
        BatchData::Int64(values) => {
            BatchData::Int64(gather_with(selection, |row| *at(values, row)))
        }
        BatchData::Float64(values) => {
            BatchData::Float64(gather_with(selection, |row| *at(values, row)))
        }
        BatchData::Utf8(values) => {
            BatchData::Utf8(gather_with(selection, |row| at(values, row).clone()))
        }
    };
    BatchColumn::with_validity(column.name.clone(), validity, data)
}

/// The columns materialised during one query execution.
///
/// The filter's column is decoded *densely*, because the comparison kernel
/// reads a dense array. Every other column is gathered straight out of its
/// chunk, so a column the plan does not read is never decoded at all.
#[derive(Debug)]
pub struct Materialised<'a> {
    /// The dataset the columns come from.
    dataset: &'a Dataset,
    /// The rows that survived the filter.
    selection: SelectionVector,
    /// The columns decoded densely, by column index.
    dense: HashMap<usize, BatchColumn>,
    /// The columns gathered by the selection, by column index.
    gathered: HashMap<usize, BatchColumn>,
}

impl<'a> Materialised<'a> {
    /// Builds an empty materialisation set over `dataset`.
    #[must_use]
    pub fn new(dataset: &'a Dataset, selection: SelectionVector) -> Self {
        Self { dataset, selection, dense: HashMap::new(), gathered: HashMap::new() }
    }

    /// Replaces the selection the columns are gathered by.
    pub fn set_selection(&mut self, selection: SelectionVector) {
        self.selection = selection;
    }

    /// The rows that survived the filter.
    #[must_use]
    pub fn selection(&self) -> &SelectionVector {
        &self.selection
    }

    /// Decodes the column at `index` densely, if it is not decoded already.
    pub fn decode_dense(&mut self, index: usize) -> &BatchColumn {
        if !self.dense.contains_key(&index) {
            let chunk =
                self.dataset.chunks.get(index).expect("a materialised column index is in range");
            note_decode(&chunk.name);
            let column = chunk.decode();
            self.dense.insert(index, column);
        }
        self.dense.get(&index).expect("the column was decoded just above")
    }

    /// The densely decoded column at `index`, if it was decoded.
    #[must_use]
    pub fn dense(&self, index: usize) -> Option<&BatchColumn> {
        self.dense.get(&index)
    }

    /// Gathers the column at `index` by the current selection.
    ///
    /// The column is decoded at most once: a column that was already
    /// decoded densely is gathered from the dense copy instead of the chunk.
    pub fn gather(&mut self, index: usize) -> &BatchColumn {
        if !self.gathered.contains_key(&index) {
            let column = if let Some(dense) = self.dense.get(&index) {
                gather_dense(dense, &self.selection)
            } else {
                let chunk = self
                    .dataset
                    .chunks
                    .get(index)
                    .expect("a materialised column index is in range");
                gather(chunk, &self.selection)
            };
            self.gathered.insert(index, column);
        }
        self.gathered.get(&index).expect("the column was gathered just above")
    }

    /// The gathered column at `index`.
    ///
    /// # Panics
    ///
    /// Panics if the column was not gathered.
    #[must_use]
    pub fn gathered_ref(&self, index: usize) -> &BatchColumn {
        self.gathered.get(&index).expect("the column was gathered before the aggregate stage")
    }

    /// The names of the columns that were decoded, in schema order.
    #[must_use]
    pub fn materialised_names(&self) -> Vec<String> {
        let mut indices: Vec<usize> = self.dense.keys().copied().collect();
        indices.extend(self.gathered.keys().copied());
        indices.sort_unstable();
        indices.dedup();
        indices
            .into_iter()
            .filter_map(|index| self.dataset.chunks.get(index).map(|chunk| chunk.name.clone()))
            .collect()
    }
}

/// Gathers the values of a column chunk.
fn gather_encoding(encoding: &Encoding, selection: &SelectionVector) -> BatchData {
    match encoding {
        Encoding::Int64 { values } => {
            BatchData::Int64(gather_with(selection, |row| *at(values, row)))
        }
        Encoding::Float64 { values } => {
            BatchData::Float64(gather_with(selection, |row| *at(values, row)))
        }
        Encoding::Utf8 { values } => {
            BatchData::Utf8(gather_with(selection, |row| at(values, row).clone()))
        }
        Encoding::Int64Dict { dict, indices } => BatchData::Int64(gather_with(selection, |row| {
            let index =
                usize::try_from(*at(indices, row)).expect("a dictionary index is not negative");
            *dict.get(index).expect("a dictionary index is in range")
        })),
        Encoding::Float64Dict { dict, indices } => {
            BatchData::Float64(gather_with(selection, |row| {
                let index =
                    usize::try_from(*at(indices, row)).expect("a dictionary index is not negative");
                *dict.get(index).expect("a dictionary index is in range")
            }))
        }
        Encoding::Utf8Dict { dict, indices } => BatchData::Utf8(gather_with(selection, |row| {
            let index =
                usize::try_from(*at(indices, row)).expect("a dictionary index is not negative");
            dict.get(index).expect("a dictionary index is in range").clone()
        })),
        Encoding::BoolRle { runs } => {
            // A run-length-encoded column is decoded densely and then
            // gathered, exactly as the core decoder does it: a run-boundary
            // lookup per row is slower than the linear scan it replaces.
            let mut dense = Vec::new();
            for &(value, count) in runs {
                let count = usize::try_from(count).expect("a run length fits in a usize");
                dense.extend(std::iter::repeat_n(i64::from(value), count));
            }
            BatchData::Int64(gather_with(selection, |row| at(&dense, row).to_owned()))
        }
    }
}

/// Gathers the validity mask of a column along with its values.
fn gather_validity(source: &NullBitmap, selection: &SelectionVector) -> NullBitmap {
    let mut words = vec![0u64; NullBitmap::words_for(selection.len())];
    let mut out = 0usize;
    selection.for_each(|row| {
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        if row < source.len() && source.is_valid(row) {
            if let Some(word) = words.get_mut(out / 64) {
                *word |= 1u64 << (out % 64);
            }
        }
        out += 1;
    });
    NullBitmap::from_words(&words, selection.len())
}

/// Collects the values a selection names.
fn gather_with<T>(selection: &SelectionVector, mut value: impl FnMut(u32) -> T) -> Vec<T> {
    let mut out = Vec::with_capacity(selection.len());
    selection.for_each(|row| out.push(value(row)));
    out
}

/// Reads `values[row]`.
///
/// The row comes from a selection vector built over these very values, so it
/// is in range by construction. `usize` is at least 32 bits on every target
/// this engine supports, so the row index cannot be truncated.
#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
fn at<T>(values: &[T], row: u32) -> &T {
    &values[row as usize]
}
