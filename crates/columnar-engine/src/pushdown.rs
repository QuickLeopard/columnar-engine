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
//! So this module implements its own [`ArrowPredicate`], whose `evaluate`
//! returns the engine's answer exactly. The equivalence is argued in
//! [ADR-0010](../../../docs/adr/0010-pushdown-equivalence.md) and checked by
//! the differential test in `crates/columnar-engine/tests/lazy_scan.rs`.
//!
//! # What the predicate sees
//!
//! Arrow hands the predicate a record batch holding exactly the columns in
//! the predicate's own projection mask -- here, one column. The predicate
//! returns a boolean per row, with no nulls: `false` means "this row does
//! not match", and the reader drops it.
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
//! The pushdown runs *inside* the Parquet reader, so its cost is otherwise
//! invisible: it is part of the scan stage's elapsed time, and nothing says
//! how much of that is the reader's decode and how much is the predicate. The
//! two have different fixes -- one is a faster reader, the other a faster
//! comparison -- so the engine measures them apart. [`PushdownMetrics`] is that
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
use parquet::arrow::arrow_reader::ArrowPredicate;
use parquet::arrow::ProjectionMask;
use parquet::schema::types::SchemaDescriptor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// What the pushed-down predicates cost.
///
/// The pushdown is invisible in the scan stage's elapsed time: it is part of
/// it, because it runs inside the reader. These numbers separate the two
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
/// evaluated inside the Parquet reader.
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

    /// Builds the reader's row filter.
    ///
    /// Each predicate gets its own projection mask over the single column it
    /// reads, so the reader fetches that column's pages to evaluate it and
    /// nothing else.
    #[must_use]
    pub fn to_row_filter(
        &self,
        descriptor: &SchemaDescriptor,
        flavor: Flavor,
    ) -> parquet::arrow::arrow_reader::RowFilter {
        let predicates: Vec<Box<dyn ArrowPredicate>> = self
            .predicates
            .iter()
            .map(|predicate| {
                let projection = ProjectionMask::roots(descriptor, [predicate.column]);
                Box::new(EnginePredicate {
                    projection,
                    name: predicate.name.clone(),
                    comparisons: predicate.comparisons.clone(),
                    flavor,
                    counter: Arc::clone(&self.counter),
                }) as Box<dyn ArrowPredicate>
            })
            .collect();
        parquet::arrow::arrow_reader::RowFilter::new(predicates)
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
struct MatchMask {
    /// The bits, LSB-first.
    words: Vec<u64>,
    /// How many rows the mask describes.
    len: usize,
}

impl MatchMask {
    /// An empty mask over `len` rows.
    fn new(len: usize) -> Self {
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
    fn set(&mut self, row: usize, matched: bool) {
        if matched {
            self.words[row / 64] |= 1u64 << (row % 64);
        }
    }

    /// The mask as an Arrow boolean array, with no nulls.
    fn into_array(self) -> BooleanArray {
        if self.len == 0 {
            return BooleanArray::new(
                BooleanBuffer::new(Buffer::from_vec(Vec::<u64>::new()), 0, 0),
                None,
            );
        }
        BooleanArray::new(BooleanBuffer::new(Buffer::from_vec(self.words), 0, self.len), None)
    }
}

/// The [`ArrowPredicate`] that reproduces the engine's filter semantics.
struct EnginePredicate {
    /// The column this predicate reads.
    projection: ProjectionMask,
    /// The column's name, used to find it in the record batch.
    name: String,
    /// The comparisons on this column, `AND`ed.
    comparisons: Vec<Comparison>,
    /// Which kernel flavor evaluates the comparisons.
    flavor: Flavor,
    /// Where the evaluation reports what it cost.
    counter: Arc<PushdownCounter>,
}

impl ArrowPredicate for EnginePredicate {
    fn projection(&self) -> &ProjectionMask {
        &self.projection
    }

    fn evaluate(&mut self, batch: RecordBatch) -> Result<BooleanArray, ArrowError> {
        let column = find_column(&batch, &self.name)?;
        let started = std::time::Instant::now();
        let result = evaluate(column.as_ref(), &self.comparisons, self.flavor);
        self.counter.record(started.elapsed(), u64::try_from(column.len()).unwrap_or(u64::MAX));
        result
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
) -> Result<BooleanArray, ArrowError> {
    // A NaN threshold matches nothing, for every operator. That is decided
    // once, here, rather than once per row. The kernel knows the rule too,
    // but deciding it here keeps the answer independent of the flavor.
    if comparisons.iter().any(|comparison| is_nan_threshold(&comparison.threshold)) {
        return Ok(MatchMask::new(column.len()).into_array());
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
) -> Result<BooleanArray, ArrowError> {
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
    Ok(MatchMask::from_words(mask, rows).into_array())
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
) -> Result<BooleanArray, ArrowError> {
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
    Ok(MatchMask::from_words(mask, rows).into_array())
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
) -> Result<BooleanArray, ArrowError> {
    let distinct = string_matches(array.values().as_ref(), comparisons)?;
    let keys = array.keys();
    let rows = keys.len();
    let indices: &[i32] = keys.values();
    let mut mask = vec![0u64; rows.div_ceil(64)];
    for (chunk_index, chunk) in indices.chunks(64).enumerate() {
        let mut word = 0u64;
        for (bit, &key) in chunk.iter().enumerate() {
            let entry = usize::try_from(key).unwrap_or(usize::MAX);
            let matched = u64::from(distinct.get(entry).copied().unwrap_or(false));
            word |= matched << bit;
        }
        if let Some(slot) = mask.get_mut(chunk_index) {
            *slot = word;
        }
    }
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
    Ok(MatchMask::from_words(mask, rows).into_array())
}

/// Evaluates the comparisons over a UTF-8 column.
fn evaluate_string(
    array: &StringArray,
    comparisons: &[Comparison],
) -> Result<BooleanArray, ArrowError> {
    // The threshold's type is an invariant of the predicate, so it is checked
    // once here rather than once per row.
    let mut bounds = Vec::with_capacity(comparisons.len());
    for comparison in comparisons {
        let ScalarValue::Utf8(bound) = &comparison.threshold else {
            return Err(mismatch(&array, comparison.threshold.data_type()));
        };
        bounds.push((comparison.op, bound.as_str()));
    }
    let mut mask = MatchMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for (op, bound) in &bounds {
            if !compare_str(value, *op, bound) {
                matched = false;
                break;
            }
        }
        mask.set(row, matched);
    }
    Ok(mask.into_array())
}

/// Evaluates the comparisons over a large UTF-8 column.
fn evaluate_large_string(
    array: &LargeStringArray,
    comparisons: &[Comparison],
) -> Result<BooleanArray, ArrowError> {
    // The threshold's type is an invariant of the predicate, so it is checked
    // once here rather than once per row.
    let mut bounds = Vec::with_capacity(comparisons.len());
    for comparison in comparisons {
        let ScalarValue::Utf8(bound) = &comparison.threshold else {
            return Err(mismatch(&array, comparison.threshold.data_type()));
        };
        bounds.push((comparison.op, bound.as_str()));
    }
    let mut mask = MatchMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for (op, bound) in &bounds {
            if !compare_str(value, *op, bound) {
                matched = false;
                break;
            }
        }
        mask.set(row, matched);
    }
    Ok(mask.into_array())
}

/// The match of every distinct value of a dictionary, in dictionary order.
fn string_matches(values: &dyn Array, comparisons: &[Comparison]) -> Result<Vec<bool>, ArrowError> {
    if let Some(array) = values.as_any().downcast_ref::<StringArray>() {
        return Ok((0..array.len())
            .map(|row| {
                comparisons.iter().all(|comparison| {
                    let ScalarValue::Utf8(bound) = &comparison.threshold else {
                        return false;
                    };
                    compare_str(array.value(row), comparison.op, bound)
                })
            })
            .collect());
    }
    if let Some(array) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok((0..array.len())
            .map(|row| {
                comparisons.iter().all(|comparison| {
                    let ScalarValue::Utf8(bound) = &comparison.threshold else {
                        return false;
                    };
                    compare_str(array.value(row), comparison.op, bound)
                })
            })
            .collect());
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

/// Compares two strings, byte-wise, as `DuckDB`'s binary collation does.
///
/// Rust's `str` ordering is byte-wise lexicographic, so the engine and
/// `DuckDB` agree on the same bytes.
fn compare_str(value: &str, op: ComparisonOp, threshold: &str) -> bool {
    match op {
        ComparisonOp::Lt => value < threshold,
        ComparisonOp::LtEq => value <= threshold,
        ComparisonOp::Gt => value > threshold,
        ComparisonOp::GtEq => value >= threshold,
        ComparisonOp::Eq => value == threshold,
        ComparisonOp::Ne => value != threshold,
    }
}

/// The error for a threshold that does not match the column's type.
fn mismatch(column: &dyn Array, threshold: DataType) -> ArrowError {
    ArrowError::InvalidArgumentError(format!(
        "the column `{}` cannot be compared against a {} threshold",
        column.data_type(),
        threshold.name()
    ))
}
