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

use arrow::array::{
    Array, BooleanArray, DictionaryArray, Float64Array, Int64Array, LargeStringArray, StringArray,
};
use arrow::buffer::{BooleanBuffer, Buffer};
use arrow::datatypes::Int32Type;
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, DataType, ScalarValue};
use parquet::arrow::arrow_reader::ArrowPredicate;
use parquet::arrow::ProjectionMask;
use parquet::schema::types::SchemaDescriptor;

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PushdownPlan {
    /// The predicates, in the order the engine will apply them.
    predicates: Vec<Pushdown>,
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

    /// Builds the reader's row filter.
    ///
    /// Each predicate gets its own projection mask over the single column it
    /// reads, so the reader fetches that column's pages to evaluate it and
    /// nothing else.
    #[must_use]
    pub fn to_row_filter(
        &self,
        descriptor: &SchemaDescriptor,
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
}

impl ArrowPredicate for EnginePredicate {
    fn projection(&self) -> &ProjectionMask {
        &self.projection
    }

    fn evaluate(&mut self, batch: RecordBatch) -> Result<BooleanArray, ArrowError> {
        let column = find_column(&batch, &self.name)?;
        evaluate(column.as_ref(), &self.comparisons)
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
fn evaluate(column: &dyn Array, comparisons: &[Comparison]) -> Result<BooleanArray, ArrowError> {
    // A NaN threshold matches nothing, for every operator. That is decided
    // once, here, rather than once per row.
    if comparisons.iter().any(|comparison| is_nan_threshold(&comparison.threshold)) {
        return Ok(MatchMask::new(column.len()).into_array());
    }

    if let Some(array) = column.as_any().downcast_ref::<Int64Array>() {
        return evaluate_int64(array, comparisons);
    }
    if let Some(array) = column.as_any().downcast_ref::<Float64Array>() {
        return evaluate_float64(array, comparisons);
    }
    if let Some(array) = column.as_any().downcast_ref::<DictionaryArray<Int32Type>>() {
        return evaluate_dictionary(array, comparisons);
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
fn evaluate_int64(
    array: &Int64Array,
    comparisons: &[Comparison],
) -> Result<BooleanArray, ArrowError> {
    let mut mask = MatchMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for comparison in comparisons {
            let ScalarValue::Int64(bound) = comparison.threshold else {
                return Err(mismatch(&array, comparison.threshold.data_type()));
            };
            if !compare_i64(value, comparison.op, bound) {
                matched = false;
                break;
            }
        }
        mask.set(row, matched);
    }
    Ok(mask.into_array())
}

/// Evaluates the comparisons over a floating-point column.
fn evaluate_float64(
    array: &Float64Array,
    comparisons: &[Comparison],
) -> Result<BooleanArray, ArrowError> {
    let mut mask = MatchMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for comparison in comparisons {
            let ScalarValue::Float64(bound) = comparison.threshold else {
                return Err(mismatch(&array, comparison.threshold.data_type()));
            };
            if !compare_f64(value, comparison.op, bound) {
                matched = false;
                break;
            }
        }
        mask.set(row, matched);
    }
    Ok(mask.into_array())
}

/// Evaluates the comparisons over a dictionary-encoded column.
///
/// One comparison per distinct value, then a map through the indices.
fn evaluate_dictionary(
    array: &DictionaryArray<Int32Type>,
    comparisons: &[Comparison],
) -> Result<BooleanArray, ArrowError> {
    let distinct = string_matches(array.values().as_ref(), comparisons)?;
    let keys = array.keys();
    let mut mask = MatchMask::new(keys.len());
    if keys.null_count() == 0 {
        for (row, &key) in keys.values().iter().enumerate() {
            let matched = usize::try_from(key)
                .ok()
                .and_then(|index| distinct.get(index).copied())
                .unwrap_or(false);
            mask.set(row, matched);
        }
    } else {
        for row in 0..keys.len() {
            if keys.is_null(row) {
                continue;
            }
            let key = keys.value(row);
            let matched = usize::try_from(key)
                .ok()
                .and_then(|index| distinct.get(index).copied())
                .unwrap_or(false);
            mask.set(row, matched);
        }
    }
    Ok(mask.into_array())
}

/// Evaluates the comparisons over a UTF-8 column.
fn evaluate_string(
    array: &StringArray,
    comparisons: &[Comparison],
) -> Result<BooleanArray, ArrowError> {
    let mut mask = MatchMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for comparison in comparisons {
            let ScalarValue::Utf8(bound) = &comparison.threshold else {
                return Err(mismatch(&array, comparison.threshold.data_type()));
            };
            if !compare_str(value, comparison.op, bound) {
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
    let mut mask = MatchMask::new(array.len());
    let has_nulls = array.null_count() > 0;
    for row in 0..array.len() {
        if has_nulls && array.is_null(row) {
            continue;
        }
        let value = array.value(row);
        let mut matched = true;
        for comparison in comparisons {
            let ScalarValue::Utf8(bound) = &comparison.threshold else {
                return Err(mismatch(&array, comparison.threshold.data_type()));
            };
            if !compare_str(value, comparison.op, bound) {
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

/// Compares two 64-bit integers.
fn compare_i64(value: i64, op: ComparisonOp, threshold: i64) -> bool {
    match op {
        ComparisonOp::Lt => value < threshold,
        ComparisonOp::LtEq => value <= threshold,
        ComparisonOp::Gt => value > threshold,
        ComparisonOp::GtEq => value >= threshold,
        ComparisonOp::Eq => value == threshold,
        ComparisonOp::Ne => value != threshold,
    }
}

/// Compares two doubles, with the engine's NaN rule.
///
/// A NaN value or a NaN threshold **never matches**, for every operator
/// including `!=`. That is stricter than IEEE-754, where `NaN != 5.0` is
/// true, and it is the rule the comparison kernel implements.
///
/// The `float_cmp` suggestion is wrong here, and deliberately overridden:
/// it suggests comparing floats "within some margin of error". In a query
/// engine that would change what the query *means*: `WHERE x = 0.1` is an
/// equality test on the stored bits, not a tolerance test. An epsilon
/// comparison would silently return different rows than `DuckDB`. The same
/// reasoning is recorded on the kernel's oracle.
#[allow(clippy::float_cmp)]
fn compare_f64(value: f64, op: ComparisonOp, threshold: f64) -> bool {
    // The threshold's own NaN is settled before the loop, so it is not
    // checked here.
    match op {
        // IEEE-754 already says every ordered comparison with a NaN value is
        // false, so only `!=` needs the explicit check.
        ComparisonOp::Lt => value < threshold,
        ComparisonOp::LtEq => value <= threshold,
        ComparisonOp::Gt => value > threshold,
        ComparisonOp::GtEq => value >= threshold,
        ComparisonOp::Eq => value == threshold,
        ComparisonOp::Ne => !value.is_nan() && value != threshold,
    }
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
