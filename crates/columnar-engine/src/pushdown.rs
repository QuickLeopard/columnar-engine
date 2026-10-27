//! Predicate pushdown into the Parquet reader.
//!
//! # Why the engine does not use Arrow's comparisons
//!
//! The engine's filter has two rules that are **stricter than SQL**, and both
//! of them are load-bearing:
//!
//! - A row whose validity bit is clear is NULL and **never matches**, for
//!   every operator, **including `!=`**.
//! - A comparison involving a NaN value or a NaN threshold **never matches**,
//!   for every operator, **including `!=`**.
//!
//! Arrow's own comparison kernels disagree on both counts:
//!
//! - Arrow returns `null` for a comparison against a NULL, and `RowFilter`
//!   keeps a row whose predicate evaluates to `null`. So Arrow would keep
//!   the NULL rows the engine drops.
//! - In IEEE-754, `NaN != 5.0` is **true**. Arrow would keep the NaN rows
//!   the engine drops.
//!
//! Pushing Arrow's semantics down would therefore answer a *different
//! question* from the one the eager path answers, and the two would disagree
//! on any dataset with a NULL or a NaN. That is not a performance question.
//!
//! So this module evaluates the comparisons itself, over Arrow's raw value
//! slice, with the engine's own kernel. The equivalence is argued in
//! [ADR-0010](../../../docs/adr/0010-pushdown-equivalence.md) and checked by
//! the differential test in `crates/columnar-engine/tests/lazy_scan.rs`.
//!
//! # Where the predicate runs
//!
//! The engine applies the predicate **itself**, over the arrays the reader
//! hands it, and keeps the mask. It does *not* hand the reader a row filter.
//!
//! That is not a style choice. A row filter is a *range list*: parquet-rs
//! turns the mask into a list of row ranges and then applies it one range at a
//! time, through one `read_records` call per range. A predicate over scattered
//! data produces a mask with a range per surviving run, and Q6's discount
//! predicate -- a random float column, so the survivors are scattered --
//! produces **2,775,138 ranges for 2,178,861 surviving rows**. Reading the
//! next column through that list costs 50.0 ms; reading it without one costs
//! 7.8 ms. The engine's own mask is one bit per row and costs nothing to
//! apply. See [ADR-0015](../../../docs/adr/0015-the-engine-keeps-the-mask.md).
//!
//! # What the predicate sees
//!
//! The predicate sees a record batch holding the columns the scan reads -- the
//! columns the query materialises, and the columns the predicates themselves
//! need. It returns a boolean per row, with no nulls: `false` means "this row
//! does not match", and the row is dropped.
//!
//! # Dictionary columns
//!
//! A dictionary-encoded column is compared **once per distinct value** and
//! then mapped back through the indices, exactly as the engine's own
//! dictionary path does. Comparing 2,500 distinct dates once is not the same
//! work as comparing 6,000,000 dates, and the engine would not accept a
//! pushdown that quietly did the second.
//!
//! # What the pushdown costs
//!
//! The pushdown runs *inside* the scan, so its cost is otherwise invisible:
//! it is part of the scan stage's elapsed time, and nothing says how much of
//! that is the reader's decode and how much is the predicate. The two have
//! different fixes -- one is a faster reader, the other a faster comparison
//! -- so the engine measures them apart. [`PushdownMetrics`] is that
//! measurement, and the plan output prints it.

use arrow::array::{
    Array, BooleanArray, DictionaryArray, Float64Array, Int64Array, LargeStringArray, StringArray,
};
use arrow::buffer::{BooleanBuffer, Buffer};
use arrow::datatypes::Int32Type;
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, DataType, NullBitmap, ScalarValue};
use columnar_kernels::Flavor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// What the pushed-down predicates cost.
///
/// The pushdown is invisible in the scan stage's elapsed time: it is part of
/// it, because it runs inside the scan. These numbers separate the two
/// halves, because they have different fixes. A slower decode is a reader
/// problem; a slower comparison is a kernel problem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushdownMetrics {
    /// The time spent evaluating the predicates.
    pub predicate_ns: u64,
    /// The number of rows the predicates were evaluated over.
    pub rows_evaluated: u64,
    /// The number of batches the predicates were evaluated over.
    pub batches: u64,
}

impl PushdownMetrics {
    /// The time spent evaluating the predicates.
    #[must_use]
    pub fn predicate_time(&self) -> Duration {
        Duration::from_nanos(self.predicate_ns)
    }

    /// The mean time per row the predicates were evaluated over, in
    /// picoseconds.
    ///
    /// Integer arithmetic, deliberately: this is a diagnostic, and a float
    /// would only add a rounding question to a figure that is already an
    /// average over millions of rows.
    ///
    /// Zero when no row was evaluated, rather than a division by zero.
    #[must_use]
    pub fn ps_per_row(&self) -> u64 {
        if self.rows_evaluated == 0 {
            return 0;
        }
        self.predicate_ns.saturating_mul(1_000) / self.rows_evaluated
    }
}

/// The accumulator the predicates write into.
///
/// The reader owns the batch it hands a predicate, so the counter is shared
/// rather than owned. It is interior-mutable and lock-free: the predicates run
/// on the reader's thread, and the totals are read once, after the read.
#[derive(Debug, Default)]
struct PushdownCounter {
    predicate_ns: AtomicU64,
    rows_evaluated: AtomicU64,
    batches: AtomicU64,
}

impl PushdownCounter {
    /// Records one predicate evaluation.
    fn record(&self, elapsed: Duration, rows: u64) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.predicate_ns.fetch_add(nanos, Ordering::Relaxed);
        self.rows_evaluated.fetch_add(rows, Ordering::Relaxed);
        self.batches.fetch_add(1, Ordering::Relaxed);
    }

    /// The totals so far.
    fn snapshot(&self) -> PushdownMetrics {
        PushdownMetrics {
            predicate_ns: self.predicate_ns.load(Ordering::Relaxed),
            rows_evaluated: self.rows_evaluated.load(Ordering::Relaxed),
            batches: self.batches.load(Ordering::Relaxed),
        }
    }
}

/// A predicate that has been checked against the file's schema and can be
/// evaluated over the reader's arrays.
///
/// A predicate belongs to a **column**, and a column may carry several
/// predicates: the engine's filter is a conjunction, and two predicates on
/// the same column are one conjunction over that column. They are pushed
/// together, in one predicate object, because the reader would otherwise
/// decode the column once per predicate -- and "Columns may be decoded
/// multiple times if they appear in multiple `ProjectionMask`" is a warning
/// worth heeding.
#[derive(Debug, Clone, PartialEq)]
pub struct Pushdown {
    /// The root index of the filtered column in the file's schema.
    pub column: usize,
    /// The column's name, used to find the column in the record batch the
    /// reader hands over.
    pub name: String,
    /// The comparisons, `AND`ed.
    pub comparisons: Vec<Comparison>,
}

/// One comparison in a pushdown.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// The comparison operator.
    pub op: ComparisonOp,
    /// The threshold the column is compared against.
    pub threshold: ScalarValue,
}

impl Pushdown {
    /// Builds a predicate to push into the reader.
    ///
    /// The plan's resolution has already checked that the threshold's type is
    /// the column's type, so this does not check it again.
    #[must_use]
    pub fn new(column: usize, name: &str, op: ComparisonOp, threshold: ScalarValue) -> Self {
        Self { column, name: name.to_owned(), comparisons: vec![Comparison { op, threshold }] }
    }

    /// Adds another comparison on the same column.
    pub fn and(&mut self, op: ComparisonOp, threshold: ScalarValue) {
        self.comparisons.push(Comparison { op, threshold });
    }

    /// A human-readable description of the predicate.
    #[must_use]
    pub fn describe(&self) -> String {
        self.comparisons
            .iter()
            .map(|comparison| {
                format!("{} {} {}", self.name, comparison.op.symbol(), comparison.threshold)
            })
            .collect::<Vec<String>>()
            .join(" and ")
    }
}

/// The set of predicates to push into one read.
#[derive(Debug, Clone, Default)]
pub struct PushdownPlan {
    /// The predicates, in the order the engine will apply them.
    predicates: Vec<Pushdown>,
    /// Where the predicates report what they cost.
    counter: Arc<PushdownCounter>,
}

/// Two plans are equal when they push the same predicates.
///
/// The cost counter is instrumentation, not part of the plan: it changes as
/// the predicates run, and two plans that push the same predicates are the
/// same plan.
impl PartialEq for PushdownPlan {
    fn eq(&self, other: &Self) -> bool {
        self.predicates == other.predicates
    }
}

impl PushdownPlan {
    /// An empty plan: nothing is pushed down.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a predicate to the plan.
    pub fn push(&mut self, predicate: Pushdown) {
        self.predicates.push(predicate);
    }

    /// Whether the plan is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.predicates.is_empty()
    }

    /// How many predicates the plan holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.predicates.len()
    }

    /// The predicates, in order.
    #[must_use]
    pub fn predicates(&self) -> &[Pushdown] {
        &self.predicates
    }

    /// The predicate on `column`, if the plan has one.
    pub fn for_column_mut(&mut self, column: usize) -> Option<&mut Pushdown> {
        self.predicates.iter_mut().find(|predicate| predicate.column == column)
    }

    /// What the pushed-down predicates have cost so far.
    ///
    /// Zero before the read, and the measurement after it.
    #[must_use]
    pub fn metrics(&self) -> PushdownMetrics {
        self.counter.snapshot()
    }

    /// The file's root indices of the columns this plan's predicates read.
    ///
    /// These columns are read because the predicates need them, not because
    /// the query materialises them. The engine evaluates the predicates over
    /// the reader's arrays and drops the columns afterwards.
    #[must_use]
    pub fn columns(&self) -> Vec<usize> {
        let mut columns: Vec<usize> =
            self.predicates.iter().map(|predicate| predicate.column).collect();
        columns.sort_unstable();
        columns.dedup();
        columns
    }

    /// Evaluates the plan's predicates over a record batch.
    ///
    /// Returns the mask of the rows that satisfy **all** of the plan's
    /// predicates, or `None` when the plan is empty and every row survives.
    ///
    /// The predicates are evaluated with the engine's own kernel over Arrow's
    /// raw value slice -- the same function the filter runs -- so the pushdown
    /// and the filter cannot disagree about which rows match.
    ///
    /// The masks of the individual predicates are `AND`ed together. They are
    /// aligned because every column of a record batch holds the same rows.
    pub fn evaluate_batch(
        &self,
        batch: &RecordBatch,
        flavor: Flavor,
    ) -> Result<Option<RowMask>, ArrowError> {
        if self.predicates.is_empty() {
            return Ok(None);
        }
        let mut combined: Option<RowMask> = None;
        for predicate in &self.predicates {
            let column = find_column(batch, &predicate.name)?;
            let started = std::time::Instant::now();
            let mask = evaluate(column.as_ref(), &predicate.comparisons, flavor)?;
            self.counter.record(started.elapsed(), u64::try_from(column.len()).unwrap_or(u64::MAX));
            combined = Some(match combined {
                None => mask,
                Some(kept) => kept.and(&mask),
            });
        }
        Ok(combined)
    }
}

/// A match mask: one bit per row, LSB-first within each 64-bit word.
///
/// This is the same layout as Arrow's `BooleanBuffer` and as the engine's own
/// `NullBitmap`, so the mask goes to Arrow without a copy.
///
/// The mask is built a word at a time rather than a bit at a time through
/// Arrow's `BooleanBuilder`. The builder checks its capacity and shifts a
/// bit per element, and at six million rows that is not free: the
/// predicate over Q6's two columns cost more than decoding them.
///
/// The mask is the engine's own, and it stays the engine's own: it is never
/// turned into a row-range list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowMask {
    /// The bits, LSB-first.
    words: Vec<u64>,
    /// How many rows the mask describes.
    len: usize,
}

impl RowMask {
    /// An empty mask over `len` rows.
    pub(crate) fn new(len: usize) -> Self {
        Self { words: vec![0u64; len.div_ceil(64)], len }
    }

    /// A mask over `len` rows, from bit-packed words.
    ///
    /// The words are LSB-first, which is the layout the kernel writes and the
    /// layout Arrow's boolean buffer uses, so no conversion is needed.
    fn from_words(words: Vec<u64>, len: usize) -> Self {
        Self { words, len }
    }

    /// Sets the bit for `row`.
    #[inline]
    pub(crate) fn set(&mut self, row: usize, matched: bool) {
        if matched {
            self.words[row / 64] |= 1u64 << (row % 64);
        }
    }

    /// The number of rows the mask keeps.
    ///
    /// The mask's last word may hold bits past the mask's length. They are
    /// zero, but the count masks them off anyway: a count that depended on
    /// the padding would be a count that depended on the allocator.
    #[must_use]
    pub fn count(&self) -> usize {
        let mut count = 0usize;
        for (index, word) in self.words.iter().enumerate() {
            let bits = self.len.saturating_sub(index * 64).min(64);
            if bits == 64 {
                count += word.count_ones() as usize;
            } else {
                count += (word & ((1u64 << bits) - 1)).count_ones() as usize;
            }
        }
        count
    }

    /// Visits the rows the mask keeps, in ascending order.
    ///
    /// This is the mask's own iteration, and it is the order every consumer
    /// of the mask has to preserve: a fold over the survivors adds them in
    /// the order they appear in the file, because a `SUM` over floats is not
    /// associative and a different order is a different answer.
    pub fn for_each(&self, mut f: impl FnMut(usize)) {
        let words = self.words.len();
        for (word_index, &word) in self.words.iter().enumerate() {
            let mut bits = word;
            if word_index + 1 == words {
                // The last word may hold bits past the mask's length. They
                // are zero, but they are masked off anyway: a row that
                // depended on the padding would be a row that depended on
                // the allocator.
                let rows = self.len.saturating_sub(word_index * 64).min(64);
                if rows < 64 {
                    bits &= (1u64 << rows) - 1;
                }
            }
            let base = word_index * 64;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                f(base + bit);
                bits &= bits - 1;
            }
        }
    }

    /// The rows the mask keeps, in ascending order.
    ///
    /// The iterator walks the mask's words, so it costs nothing per row it
    /// does not keep: a mask over six million rows is 93,000 words, and the
    /// rows a filter keeps are found by walking them.
    #[must_use]
    pub fn positions(&self) -> MaskPositions<'_> {
        MaskPositions { mask: self, word_index: 0, bits: 0 }
    }

    /// The mask as an Arrow boolean array, with no nulls.
    #[must_use]
    pub fn into_array(self) -> BooleanArray {
        if self.len == 0 {
            return BooleanArray::new(
                BooleanBuffer::new(Buffer::from_vec(Vec::<u64>::new()), 0, 0),
                None,
            );
        }
        BooleanArray::new(BooleanBuffer::new(Buffer::from_vec(self.words), 0, self.len), None)
    }

    /// The rows this mask keeps and `other` keeps.
    pub(crate) fn and(mut self, other: &Self) -> Self {
        and_words(&mut self.words, &other.words);
        self
    }
}

/// The rows a mask keeps, in ascending order.
///
/// The rows are found a word at a time, and the words past the mask's length
/// are masked off, so a row never depends on the padding.
#[derive(Debug, Clone)]
pub struct MaskPositions<'a> {
    /// The mask being walked.
    mask: &'a RowMask,
    /// The word the walk is in.
    word_index: usize,
    /// The bits of that word that are still to come.
    bits: u64,
}

impl Iterator for MaskPositions<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        while self.bits == 0 {
            let word = *self.mask.words.get(self.word_index)?;
            let base = self.word_index * 64;
            let rows = self.mask.len.saturating_sub(base).min(64);
            self.word_index += 1;
            self.bits = if rows < 64 { word & ((1u64 << rows) - 1) } else { word };
        }
        let bit = self.bits.trailing_zeros() as usize;
        self.bits &= self.bits - 1;
        Some(u32::try_from((self.word_index - 1) * 64 + bit).unwrap_or(u32::MAX))
    }
}

/// Finds a column in the batch by name.
fn find_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a arrow::array::ArrayRef, ArrowError> {
    batch
        .schema()
        .fields()
        .iter()
        .position(|field| field.name() == name)
        .and_then(|index| batch.columns().get(index))
        .ok_or_else(|| ArrowError::SchemaError(format!("the record batch has no column `{name}`")))
}

/// Evaluates a conjunction of comparisons over one column, with the
/// engine's semantics.
///
/// The result has no nulls: `false` means the row does not match.
fn evaluate(
    column: &dyn Array,
    comparisons: &[Comparison],
    flavor: Flavor,
) -> Result<RowMask, ArrowError> {
    // A NaN threshold matches nothing, for every operator. That is decided
    // once, here, rather than once per row. The kernel knows the rule too,
    // but deciding it here keeps the answer independent of the flavor.
    if comparisons.iter().any(|comparison| is_nan_threshold(&comparison.threshold)) {
        return Ok(RowMask::new(column.len()));
    }

    if let Some(array) = column.as_any().downcast_ref::<Int64Array>() {
        return evaluate_int64(array, comparisons, flavor);
    }
    if let Some(array) = column.as_any().downcast_ref::<Float64Array>() {
        return evaluate_float64(array, comparisons, flavor);
    }
    if let Some(array) = column.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        return evaluate_dictionary(array, comparisons, flavor);
    }
    if let Some(array) = column.as_any().downcast_ref::<StringArray>() {
        return evaluate_string(array, comparisons);
    }
    if let Some(array) = column.as_any().downcast_ref::<LargeStringArray>() {
        return evaluate_large_string(array, comparisons);
    }

    Err(ArrowError::InvalidArgumentError(format!(
        "the pushdown does not implement the column type `{}`",
        column.data_type()
    )))
}

/// Evaluates the comparisons over an integer column.
///
/// The comparisons run through the **engine's own kernel**, over Arrow's raw
/// value slice. That is not only an optimisation: it is what makes the
/// pushdown and the filter the *same function*. The kernel's contract **is**
/// the engine's comparison semantics -- a NULL never matches, a NaN never
/// matches, `-0.0` and `+0.0` compare equal -- and the filter already runs
/// it. A pushdown running its own comparisons could agree with the filter
/// only by luck; this one cannot disagree with it at all.
fn evaluate_int64(
    array: &Int64Array,
    comparisons: &[Comparison],
    flavor: Flavor,
) -> Result<RowMask, ArrowError> {
    let rows = array.len();
    let validity = validity_of(array);
    let mut mask = vec![0u64; rows.div_ceil(64)];
    let mut first = true;
    for comparison in comparisons {
        let ScalarValue::Int64(bound) = comparison.threshold else {
            return Err(mismatch(&array, comparison.threshold.data_type()));
        };
        let mut word = vec![0u64; rows.div_ceil(64)];
        columnar_kernels::compare_i64_into(
            flavor,
            array.values(),
            comparison.op,
            bound,
            validity.as_ref(),
            &mut word,
        );
        if first {
            mask.copy_from_slice(&word);
            first = false;
        } else {
            and_words(&mut mask, &word);
        }
    }
    Ok(RowMask::from_words(mask, rows))
}

/// Evaluates the comparisons over a floating-point column.
///
/// See [`evaluate_int64`] for why this runs the kernel rather than a
/// comparison of its own. The NaN rule comes with it: a comparison involving
/// a NaN value or a NaN threshold never matches, for every operator.
fn evaluate_float64(
    array: &Float64Array,
    comparisons: &[Comparison],
    flavor: Flavor,
) -> Result<RowMask, ArrowError> {
    let rows = array.len();
    let validity = validity_of(array);
    let mut mask = vec![0u64; rows.div_ceil(64)];
    let mut first = true;
    for comparison in comparisons {
        let ScalarValue::Float64(bound) = comparison.threshold else {
            return Err(mismatch(&array, comparison.threshold.data_type()));
        };
        let mut word = vec![0u64; rows.div_ceil(64)];
        columnar_kernels::compare_f64_into(
            flavor,
            array.values(),
            comparison.op,
            bound,
            validity.as_ref(),
            &mut word,
        );
        if first {
            mask.copy_from_slice(&word);
            first = false;
        } else {
            and_words(&mut mask, &word);
        }
    }
    Ok(RowMask::from_words(mask, rows))
}

/// The conjunction of two match masks.
fn and_words(mask: &mut [u64], other: &[u64]) {
    for (kept, computed) in mask.iter_mut().zip(other.iter()) {
        *kept &= computed;
    }
}

/// The validity of an Arrow array, in the engine's layout.
///
/// Arrow's null buffer and the engine's `NullBitmap` are both bit-packed and
/// LSB-first, with a set bit meaning "this row holds a value", so the common
/// case is a straight copy. Arrow's buffer may start at a bit offset, which
/// the engine's does not, so an offset buffer is shifted down into place.
fn validity_of(array: &dyn Array) -> Option<NullBitmap> {
    let nulls = array.nulls()?;
    let len = nulls.len();
    let buffer = nulls.inner();
    let offset = buffer.offset();
    let bytes = buffer.values();
    let mut words = vec![0u64; NullBitmap::words_for(len)];
    if offset % 8 == 0 {
        // Arrow packs a bitmap into `u64` words in little-endian byte order,
        // and the buffer is a whole number of *bytes* rather than of words --
        // a small array's null buffer is one byte long. The words are
        // assembled here.
        let start = offset / 8;
        for (index, chunk) in bytes[start.min(bytes.len())..].chunks(8).enumerate() {
            let mut buffer = [0u8; 8];
            buffer[..chunk.len()].copy_from_slice(chunk);
            words[index] = u64::from_le_bytes(buffer);
        }
    } else {
        // A bit offset: the bits are shifted down into place, one at a time.
        // The Parquet reader never produces one; this is the path that keeps
        // the answer right if something else does.
        for bit in 0..len {
            let source = offset + bit;
            let (byte_index, bit_index) = (source / 8, source % 8);
            let set = bytes.get(byte_index).is_some_and(|byte| (byte >> bit_index) & 1 == 1);
            if set {
                words[bit / 64] |= 1u64 << (bit % 64);
            }
        }
    }
    Some(NullBitmap::from_words(&words, len))
}

/// Evaluates the comparisons over a dictionary-encoded column.
///
/// One comparison per distinct value, then a map through the indices.
///
/// The map is branchless and accumulates a word at a time, exactly as the
/// engine's own dictionary path does: a date column's dictionary indices run
/// in long stretches, so a conditional bit set is a mispredicted branch every
/// time the run ends. The engine's path was rewritten for that reason and
/// cost 41 ms before it was fixed; the pushdown pays the same price for the
/// same mistake if it is written the obvious way.
fn evaluate_dictionary(
    array: &DictionaryArray<Int32Type>,
    comparisons: &[Comparison],
    flavor: Flavor,
) -> Result<RowMask, ArrowError> {
    let distinct = string_matches(array.values().as_ref(), comparisons)?;
    let keys = array.keys();
    let rows = keys.len();
    let indices: &[i32] = keys.values();
    let mut mask = crate::dictionary::map_mask(&distinct, indices, rows);
    // A NULL row is NULL regardless of what its dictionary entry says, so the
    // validity mask is applied to the answer rather than to the lookup.
    if let Some(validity) = validity_of(array) {
        for (word_index, word) in mask.iter_mut().enumerate() {
            if let Some(validity_word) = validity.words().get(word_index) {
                *word &= validity_word;
            }
        }
    }
    let _ = flavor;
    Ok(RowMask::from_words(mask, rows))
}

/// Evaluates the comparisons over a UTF-8 column.
fn evaluate_string(array: &StringArray, comparisons: &[Comparison]) -> Result<RowMask, ArrowError> {
    // The threshold's type is an invariant of the predicate, so it is checked
    // once here rather than once per row.
    let mut bounds = Vec::with_capacity(comparisons.len());
    for comparison in comparisons {
        let ScalarValue::Utf8(bound) = &comparison.threshold else {
            return Err(mismatch(&array, comparison.threshold.data_type()));
        };
        bounds.push((comparison.op, bound.as_str()));
    }
    let mut mask = RowMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for (op, bound) in &bounds {
            if !crate::dictionary::compare_str(value, *op, bound) {
                matched = false;
                break;
            }
        }
        mask.set(row, matched);
    }
    Ok(mask)
}

/// Evaluates the comparisons over a large UTF-8 column.
fn evaluate_large_string(
    array: &LargeStringArray,
    comparisons: &[Comparison],
) -> Result<RowMask, ArrowError> {
    // The threshold's type is an invariant of the predicate, so it is checked
    // once here rather than once per row.
    let mut bounds = Vec::with_capacity(comparisons.len());
    for comparison in comparisons {
        let ScalarValue::Utf8(bound) = &comparison.threshold else {
            return Err(mismatch(&array, comparison.threshold.data_type()));
        };
        bounds.push((comparison.op, bound.as_str()));
    }
    let mut mask = RowMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for (op, bound) in &bounds {
            if !crate::dictionary::compare_str(value, *op, bound) {
                matched = false;
                break;
            }
        }
        mask.set(row, matched);
    }
    Ok(mask)
}

/// The match of every distinct value of a dictionary, in dictionary order.
fn string_matches(values: &dyn Array, comparisons: &[Comparison]) -> Result<Vec<bool>, ArrowError> {
    // The bounds are resolved **once**, before the loop over the values: a
    // comparison per distinct value must not match on an enum per distinct
    // value.
    let mut bounds: Vec<(ComparisonOp, &str)> = Vec::with_capacity(comparisons.len());
    for comparison in comparisons {
        let ScalarValue::Utf8(bound) = &comparison.threshold else {
            return Err(ArrowError::InvalidArgumentError(format!(
                "the pushdown does not implement the dictionary value type `{}`",
                values.data_type()
            )));
        };
        bounds.push((comparison.op, bound.as_str()));
    }
    if let Some(array) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(crate::dictionary::distinct_matches(array.iter(), &bounds));
    }
    if let Some(array) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(crate::dictionary::distinct_matches(array.iter(), &bounds));
    }
    Err(ArrowError::InvalidArgumentError(format!(
        "the pushdown does not implement the dictionary value type `{}`",
        values.data_type()
    )))
}

/// Whether the threshold is a NaN.
fn is_nan_threshold(threshold: &ScalarValue) -> bool {
    matches!(threshold, ScalarValue::Float64(value) if value.is_nan())
}

/// The error for a threshold that does not match the column's type.
fn mismatch(column: &dyn Array, threshold: DataType) -> ArrowError {
    ArrowError::InvalidArgumentError(format!(
        "the column `{}` cannot be compared against a {} threshold",
        column.data_type(),
        threshold.name()
    ))
}

#[cfg(test)]
mod tests {
    use super::RowMask;

    /// The mask's own iteration visits the kept rows, and only those, in
    /// ascending order.
    ///
    /// The order is not a convenience: a fold over the survivors adds them in
    /// the order they appear in the file, because a `SUM` over floats is not
    /// associative. A mask that visited its rows in another order would make
    /// the answer depend on the order the bits happened to be walked in.
    #[test]
    fn the_mask_visits_the_kept_rows_in_ascending_order() {
        let mask = RowMask::from_words(vec![0b1010_1010, 0b0000_0001], 128);
        let mut visited = Vec::new();
        mask.for_each(|row| visited.push(row));

        assert_eq!(visited, vec![1, 3, 5, 7, 64], "the mask visited the wrong rows");
    }

    /// The mask's last word may hold bits past the mask's length. They are
    /// zero, but the iteration masks them off anyway: a row that depended on
    /// the padding would be a row that depended on the allocator.
    #[test]
    fn the_mask_does_not_visit_the_padding() {
        // 65 rows: the first word holds all of them but one, and the second
        // word holds a bit at row 67 -- past the end of the mask.
        let mask = RowMask::from_words(vec![u64::MAX, 0b1000], 65);
        assert_eq!(mask.count(), 64, "the count saw the padding");

        let mut visited = Vec::new();
        mask.for_each(|row| visited.push(row));
        assert_eq!(visited.len(), 64, "the iteration visited the padding");
        assert!(visited.iter().all(|&row| row < 65), "the iteration visited a row past the end");
    }

    /// An empty mask visits nothing.
    #[test]
    fn an_empty_mask_visits_nothing() {
        let mask = RowMask::from_words(Vec::new(), 0);
        let mut visited = 0usize;
        mask.for_each(|_| visited += 1);
        assert_eq!(visited, 0, "an empty mask visited a row");
    }
}
