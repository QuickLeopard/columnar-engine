//! The filter operator.
//!
//! The filter evaluates a **conjunction** of predicates over a dataset and
//! produces the selection vector of the rows that satisfy all of them.
//!
//! ## Three implementations of one predicate
//!
//! A predicate over a column is evaluated by whichever path fits the
//! column's encoding:
//!
//! | Column | Path | Work |
//! |---|---|---|
//! | `int64` / `float64`, plain | the SIMD comparison kernel | one pass over the rows |
//! | `int64` / `float64`, dictionary | the kernel over the **dictionary**, then a map | one pass over the *distinct values* |
//! | `utf8`, dictionary | compare each distinct value once, then a map | one pass over the *distinct values* |
//! | `utf8`, plain | compare each row's string | one pass over the rows |
//!
//! The dictionary paths are the interesting ones. A dictionary-encoded column
//! holds one index per row and one value per distinct entry, so a predicate
//! over it can be evaluated **once per distinct value** and then mapped
//! through the indices. On the TPC-H `lineitem` schema that is the difference
//! between comparing 6,000,000 dates and comparing the 2,500 distinct dates
//! that appear among them.
//!
//! ## The rules the filter inherits from the kernel
//!
//! - A row whose validity bit is clear is NULL and **never matches**, for
//!   every operator, including `!=`.
//! - A comparison involving a NaN **never matches**, for every operator,
//!   including `!=`.
//!
//! Both are stricter than SQL, and both are what makes the null path
//! branchless: the values array is dense and always holds a real value, and
//! the validity mask says which of those values the predicate may look at.
//!
//! ## String ordering
//!
//! String comparison is byte-wise lexicographic, which is what Rust's `str`
//! ordering does and what `DuckDB`'s default binary collation does. The two
//! therefore agree on the same bytes, which is what makes the external
//! comparison a comparison.

use columnar_core::{
    BatchColumn, BatchData, ComparisonOp, Encoding, NullBitmap, Predicate, ScalarValue,
    SelectionKind, SelectionVector,
};
use columnar_kernels::Flavor;

use crate::plan::ResolvedFilter;
use crate::{Dataset, EngineError};

/// What the filter did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterStats {
    /// Rows entering the filter.
    pub rows_in: u64,
    /// Rows leaving the filter.
    pub rows_out: u64,
    /// The representation the selection vector settled on.
    ///
    /// The engine picks between an index list and a bitmap by density; see
    /// [`columnar_core::BITMAP_DISPATCH_DENSITY`].
    pub kind: SelectionKind,
    /// How many workers the filter ran.
    ///
    /// One for the serial filter. For the parallel one it is the number of
    /// row blocks that were computed, which is the number of workers that
    /// actually did something -- not the number of threads that were offered.
    pub workers: usize,
}

/// Filters `dataset` by a conjunction of predicates.
///
/// Every predicate must hold for a row to survive. The predicates are
/// evaluated independently and their masks are `AND`ed, so the cost is the sum
/// of the predicates' costs and the result is the intersection.
///
/// # Errors
///
/// Returns [`EngineError::TypeMismatch`] when a predicate's threshold does not
/// have the column's type.
pub fn filter_all(
    dataset: &Dataset,
    filters: &[ResolvedFilter],
    flavor: Flavor,
) -> Result<(SelectionVector, FilterStats), EngineError> {
    filter_all_with_workers(dataset, filters, flavor, 1)
}

/// Filters `dataset` by a conjunction of predicates, over `workers` threads.
///
/// The rows are split into **contiguous, word-aligned blocks**, one per
/// worker, and each worker computes the conjunction's match mask for its own
/// block. The masks are then concatenated **in block order**.
///
/// Both halves of that sentence matter. Contiguous blocks mean the workers
/// write to disjoint parts of the output, and word-aligned blocks mean each
/// block's mask is a whole number of words, so the concatenation is a
/// concatenation and not a shift. Merging the blocks in *completion* order
/// would make the selection depend on the scheduler -- and the selection is
/// the set of rows the rest of the query sees.
///
/// The result is **bit-identical** to the serial filter's: the same mask, the
/// same rows, in the same order.
///
/// # Errors
///
/// Returns [`EngineError::TypeMismatch`] when a predicate's threshold does not
/// have the column's type.
pub fn filter_all_with_workers(
    dataset: &Dataset,
    filters: &[ResolvedFilter],
    flavor: Flavor,
    workers: usize,
) -> Result<(SelectionVector, FilterStats), EngineError> {
    let rows = dataset.len();
    let workers = workers.max(1);

    if workers > 1 && rows > BLOCK_ROWS {
        let blocks = block_ranges(rows, workers);
        let block_count = blocks.len();
        let masks = std::thread::scope(|scope| {
            let handles: Vec<_> = blocks
                .iter()
                .map(|&(start, end)| {
                    scope.spawn(move || filter_block(dataset, filters, flavor, start, end))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().map_err(|_| {
                        EngineError::UnknownColumn("a filter worker panicked".to_owned())
                    })?
                })
                .collect::<Result<Vec<Vec<u64>>, EngineError>>()
        })?;

        let mut combined: Vec<u64> = Vec::with_capacity(rows.div_ceil(64));
        for mask in masks {
            combined.extend_from_slice(&mask);
        }
        let combined = trim_to_rows(combined, rows);
        let matched = combined.iter().map(|word| word.count_ones() as usize).sum::<usize>();
        let selection = SelectionVector::from_filter(&combined, matched, rows);
        let kind = selection.kind();
        return Ok((
            selection,
            FilterStats {
                rows_in: u64::try_from(rows).unwrap_or(u64::MAX),
                rows_out: u64::try_from(matched).unwrap_or(u64::MAX),
                kind,
                workers: block_count,
            },
        ));
    }

    let mut combined: Option<Vec<u64>> = None;

    for filter in filters {
        let chunk = dataset
            .chunks
            .get(filter.column)
            .ok_or_else(|| EngineError::UnknownColumn(format!("column index {}", filter.column)))?;
        let mask = mask_for_chunk(chunk, &filter.predicate, flavor, rows)?;

        combined = Some(match combined {
            None => mask,
            Some(mut accumulated) => {
                for (slot, &bits) in accumulated.iter_mut().zip(mask.iter()) {
                    *slot &= bits;
                }
                accumulated
            }
        });
    }

    let Some(combined) = combined else {
        // No predicate: every row survives.
        let selection = SelectionVector::all(rows);
        return Ok((
            selection,
            FilterStats {
                rows_in: u64::try_from(rows).unwrap_or(u64::MAX),
                rows_out: u64::try_from(rows).unwrap_or(u64::MAX),
                kind: SelectionKind::Bitmap,
                workers: 1,
            },
        ));
    };

    let matched = combined.iter().map(|word| word.count_ones() as usize).sum::<usize>();
    let selection = SelectionVector::from_filter(&combined, matched, rows);
    let stats = FilterStats {
        rows_in: u64::try_from(rows).unwrap_or(u64::MAX),
        rows_out: u64::try_from(matched).unwrap_or(u64::MAX),
        kind: selection.kind(),
        workers: 1,
    };
    Ok((selection, stats))
}

/// The rows one filter worker is given.
///
/// The blocks are **contiguous** and their starts are multiples of
/// [`BLOCK_ROWS`], so every block's mask is a whole number of words and the
/// masks concatenate without a shift.
fn block_ranges(rows: usize, workers: usize) -> Vec<(usize, usize)> {
    let per_block = rows.div_ceil(workers).div_ceil(BLOCK_ROWS) * BLOCK_ROWS;
    let mut blocks = Vec::with_capacity(workers);
    let mut start = 0;
    while start < rows {
        let end = (start + per_block).min(rows);
        blocks.push((start, end));
        start = end;
    }
    blocks
}

/// The granularity of a filter block, in rows.
///
/// Sixty-four, because a match mask is a bit per row and a mask that is a
/// whole number of words is a mask that can be concatenated with another one
/// without shifting its bits.
const BLOCK_ROWS: usize = 64;

/// Computes the conjunction's match mask over one block of rows.
fn filter_block(
    dataset: &Dataset,
    filters: &[ResolvedFilter],
    flavor: Flavor,
    start: usize,
    end: usize,
) -> Result<Vec<u64>, EngineError> {
    let mut combined: Option<Vec<u64>> = None;

    for filter in filters {
        let chunk = dataset
            .chunks
            .get(filter.column)
            .ok_or_else(|| EngineError::UnknownColumn(format!("column index {}", filter.column)))?;
        // The block's validity mask, copied: a validity mask is packed with no
        // offset, so the bits of a range have to be moved into a new mask.
        // The copy is a word at a time, so it is a memory copy rather than a
        // per-row loop.
        let validity = if chunk.validity.null_count() == 0 {
            None
        } else {
            Some(chunk.validity.slice(start, end))
        };
        let mask =
            mask_for_chunk_range(chunk, &filter.predicate, flavor, start, end, validity.as_ref())?;

        combined = Some(match combined {
            None => mask,
            Some(mut accumulated) => {
                for (slot, &bits) in accumulated.iter_mut().zip(mask.iter()) {
                    *slot &= bits;
                }
                accumulated
            }
        });
    }

    Ok(combined.unwrap_or_default())
}

/// Trims a mask to the rows it describes.
///
/// The last word of the last block may carry padding bits, because the last
/// block is not a whole number of rows.
fn trim_to_rows(mut mask: Vec<u64>, rows: usize) -> Vec<u64> {
    let remainder = rows % 64;
    if remainder != 0 {
        if let Some(last) = mask.last_mut() {
            *last &= (1u64 << remainder) - 1;
        }
    }
    mask.truncate(rows.div_ceil(64));
    mask
}

/// Produces the match mask of one predicate over one chunk.
fn mask_for_chunk(
    chunk: &columnar_core::ColumnChunk,
    predicate: &Predicate,
    flavor: Flavor,
    rows: usize,
) -> Result<Vec<u64>, EngineError> {
    // The whole chunk's validity, borrowed. The serial path never copies a
    // mask it does not have to.
    let validity = if chunk.validity.null_count() == 0 { None } else { Some(&chunk.validity) };
    mask_for_chunk_range(chunk, predicate, flavor, 0, rows, validity)
}

/// Produces the match mask of one predicate over the rows `start..end` of one
/// chunk.
///
/// The chunk's rows are **sliced**, not copied: the values, the dictionary
/// indices and the validity mask are all addressed from `start`. The validity
/// mask is the one thing the caller has to copy, because a validity mask is a
/// packed bitmap with no offset — and that copy is a word at a time.
///
/// The caller owns the copy, because it is the only thing here that needs
/// one, and the serial path -- which addresses the whole chunk -- borrows
/// instead.
fn mask_for_chunk_range(
    chunk: &columnar_core::ColumnChunk,
    predicate: &Predicate,
    flavor: Flavor,
    start: usize,
    end: usize,
    validity: Option<&NullBitmap>,
) -> Result<Vec<u64>, EngineError> {
    let rows = end - start;

    match (&chunk.encoding, &predicate.value) {
        (Encoding::Int64 { values }, ScalarValue::Int64(threshold)) => Ok(numeric_mask_i64(
            slice_of(values, start, end),
            predicate.op,
            *threshold,
            validity,
            rows,
            flavor,
        )),
        (Encoding::Float64 { values }, ScalarValue::Float64(threshold)) => Ok(numeric_mask_f64(
            slice_of(values, start, end),
            predicate.op,
            *threshold,
            validity,
            rows,
            flavor,
        )),
        (Encoding::Int64Dict { dict, indices }, ScalarValue::Int64(threshold)) => {
            // The kernel runs over the dictionary, not over the rows.
            let dict_mask =
                numeric_mask_i64(dict, predicate.op, *threshold, None, dict.len(), flavor);
            Ok(map_dictionary(&dict_mask, slice_of(indices, start, end), rows, validity))
        }
        (Encoding::Float64Dict { dict, indices }, ScalarValue::Float64(threshold)) => {
            let dict_mask =
                numeric_mask_f64(dict, predicate.op, *threshold, None, dict.len(), flavor);
            Ok(map_dictionary(&dict_mask, slice_of(indices, start, end), rows, validity))
        }
        (Encoding::Utf8Dict { dict, indices }, ScalarValue::Utf8(threshold)) => {
            // One comparison per distinct value, then a map.
            let matches: Vec<bool> = dict
                .iter()
                .map(|v| crate::dictionary::compare_str(v, predicate.op, threshold))
                .collect();
            let dict_mask = mask_from_bools(&matches);
            Ok(map_dictionary(&dict_mask, slice_of(indices, start, end), rows, validity))
        }
        (Encoding::Utf8 { values }, ScalarValue::Utf8(threshold)) => {
            let matches: Vec<bool> = slice_of(values, start, end)
                .iter()
                .map(|v| crate::dictionary::compare_str(v, predicate.op, threshold))
                .collect();
            let mask = mask_from_bools(&matches);
            Ok(apply_validity(mask, validity, rows))
        }
        (Encoding::BoolRle { .. }, _) => Err(EngineError::TypeMismatch(
            "boolean columns cannot be compared by this engine".to_owned(),
        )),
        _ => Err(EngineError::TypeMismatch(format!(
            "column `{}` is {} but the predicate compares it against a {} threshold",
            chunk.name,
            chunk.data_type().name(),
            predicate.value.data_type().name()
        ))),
    }
}

/// The rows `start..end` of a slice.
///
/// The slice is borrowed, not copied: the values are addressed from `start`,
/// so a block's mask is computed over the block's own values.
fn slice_of<T>(values: &[T], start: usize, end: usize) -> &[T] {
    values.get(start..end).unwrap_or(&[])
}

/// The match mask of an `int64` comparison.
fn numeric_mask_i64(
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
    rows: usize,
    flavor: Flavor,
) -> Vec<u64> {
    let mut words = vec![0u64; rows.div_ceil(64)];
    columnar_kernels::compare_i64_into(flavor, values, op, threshold, validity, &mut words);
    words
}

/// The match mask of a `float64` comparison.
fn numeric_mask_f64(
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    rows: usize,
    flavor: Flavor,
) -> Vec<u64> {
    let mut words = vec![0u64; rows.div_ceil(64)];
    columnar_kernels::compare_f64_into(flavor, values, op, threshold, validity, &mut words);
    words
}

/// Maps a dictionary-level mask onto the rows that point at it.
///
/// Row `r` matches when the dictionary entry its index points at matched.
/// The chunk's validity mask is applied afterwards, because a NULL row is
/// NULL regardless of what its dictionary entry says.
///
/// The dictionary's bit mask is expanded into one flag per entry first. The
/// alternative -- testing a bit inside a word, with a bounds check, once per
/// row -- was what this function used to do, and it cost 41 ms on the Q6
/// predicate set, which is six million lookups. The table is small enough to
/// live in the first-level cache, so the per-row work becomes a plain index.
///
/// The mask is then accumulated a word at a time, so that the row loop does
/// not read-modify-write the output on every row.
fn map_dictionary(
    dict_mask: &[u64],
    indices: &[i32],
    rows: usize,
    validity: Option<&NullBitmap>,
) -> Vec<u64> {
    let entry_matches = crate::dictionary::mask_to_bools(dict_mask);
    let mask = crate::dictionary::map_mask(&entry_matches, indices, rows);
    apply_validity(mask, validity, rows)
}

/// ANDs a mask with a validity mask.
fn apply_validity(mut mask: Vec<u64>, validity: Option<&NullBitmap>, rows: usize) -> Vec<u64> {
    if let Some(validity) = validity {
        for (word_index, word) in mask.iter_mut().enumerate() {
            if let Some(validity_word) = validity.words().get(word_index) {
                *word &= validity_word;
            }
        }
    }
    mask.truncate(rows.div_ceil(64));
    mask
}

/// Builds a bit mask from a list of per-element results.
fn mask_from_bools(values: &[bool]) -> Vec<u64> {
    let mut mask = vec![0u64; values.len().div_ceil(64)];
    for (index, &value) in values.iter().enumerate() {
        if value {
            mask[index / 64] |= 1u64 << (index % 64);
        }
    }
    mask
}

/// Runs one predicate over an already-decoded column.
///
/// This is the numeric path, exposed for the tests and for callers that hold
/// a decoded column rather than a dataset. The production path is
/// [`filter_all`], which works from the chunk and can use the dictionary fast
/// paths.
///
/// # Errors
///
/// Returns [`EngineError::TypeMismatch`] when the predicate's threshold does not
/// have the column's type, or when the column is a string column.
pub fn filter(
    column: &BatchColumn,
    predicate: &Predicate,
    flavor: Flavor,
) -> Result<(SelectionVector, FilterStats), EngineError> {
    let rows = column.data.len();
    let validity = if column.validity.null_count() == 0 { None } else { Some(&column.validity) };

    let mut words = vec![0u64; rows.div_ceil(64)];
    let matched = match (&column.data, &predicate.value) {
        (BatchData::Int64(values), ScalarValue::Int64(threshold)) => {
            columnar_kernels::compare_i64_into(
                flavor,
                values,
                predicate.op,
                *threshold,
                validity,
                &mut words,
            )
        }
        (BatchData::Float64(values), ScalarValue::Float64(threshold)) => {
            columnar_kernels::compare_f64_into(
                flavor,
                values,
                predicate.op,
                *threshold,
                validity,
                &mut words,
            )
        }
        (BatchData::Utf8(_), _) => {
            return Err(EngineError::TypeMismatch(format!(
                "the comparison kernels cover int64 and float64 columns only, not the utf8 column `{}`",
                column.name
            )));
        }
        _ => {
            return Err(EngineError::TypeMismatch(format!(
                "column `{}` is {} but the predicate compares it against a {} threshold",
                column.name,
                column.data_type().name(),
                predicate.value.data_type().name()
            )));
        }
    };

    let selection = SelectionVector::from_filter(&words, matched, rows);
    let stats = FilterStats {
        rows_in: u64::try_from(rows).unwrap_or(u64::MAX),
        rows_out: u64::try_from(matched).unwrap_or(u64::MAX),
        kind: selection.kind(),
        workers: 1,
    };
    Ok((selection, stats))
}
