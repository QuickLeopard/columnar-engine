//! The hash aggregate operator.
//!
//! The aggregate groups the surviving rows by the group-by columns and
//! computes each declared aggregate over every group.
//!
//! ## Determinism
//!
//! The output order is deterministic: the groups are sorted by their key
//! before they are returned, so two runs of the same plan over the same data
//! return the same groups in the same order, whatever the hash seed is. The
//! sort key is the group key values, in `group_by` order: integers compare
//! numerically, floats compare with `total_cmp` (a total order over every
//! bit pattern, so NaN is ordered deterministically rather than refused),
//! and strings compare lexicographically by byte.
//!
//! ## The sum of floats
//!
//! `SUM` over a `float64` column accumulates in row order, and floating-point
//! addition is not associative, so the result **depends on the input row
//! order**. It does not depend on the hash seed or on the order the groups
//! are visited in. The rows of a group are visited in ascending row order,
//! which is the order they appear in the input.
//!
//! `SUM` over an `int64` column saturates at the `i64` bounds rather than
//! wrapping, so an overflowing sum is loud rather than wrong.
//!
//! ## NULL group keys
//!
//! A row whose group key contains a NULL belongs to no group: the output
//! type has no way to represent a NULL key, and fabricating one (turning it
//! into a zero, say) would be a lie. Such rows are dropped from the
//! aggregation, and `COUNT(*)` counts the rows that remain.
//!
//! ## `COUNT(*)` is the in-place fast path
//!
//! `COUNT(*)` reads no column at all, so a plan that only counts rows
//! materialises nothing: no column is decoded, and the engine touches no
//! data beyond counting the rows that survived the filter.
//!
//! ## The global aggregate reads the source in place
//!
//! A **global** aggregate -- one with no GROUP BY -- has one group, and every
//! surviving row belongs to it. Such an aggregate never needs the survivors'
//! values *aligned* with the survivors: it folds them, one after another. So
//! the engine does not build a column of the survivors at all. It hands the
//! aggregate the **source** column and the **selection**, and the aggregate
//! reads the survivors' values out of the source where they lie.
//!
//! That is the whole difference between a gather and a fold: the gather
//! copies the survivors' values into a column of their own, and the fold
//! reads them where they are. The values, the order and the NULL rules are
//! the same -- a float sum is bit for bit the sum the gathered column
//! produced, because the survivors are visited in the same ascending order --
//! but the copy is not made, and a copy of a million doubles is not free.
//!
//! A **grouped** aggregate still gathers. Its group keys have to line up with
//! its values, and the hash table wants both. There is no measurement behind
//! fusing that, so it is not done.

use std::cmp::Ordering;
use std::collections::HashMap;

use columnar_core::{
    BatchColumn, BatchData, ColumnChunk, Encoding, NullBitmap, ScalarValue, SelectionVector,
};

use crate::explain::{PlanOutput, StageStats};
use crate::plan::{Aggregate, AggregateSpec};

/// The result of executing a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateResult {
    /// The groups. Empty when the plan has no aggregation.
    pub groups: Vec<Group>,
    /// Per-stage statistics, in execution order.
    pub stages: Vec<StageStats>,
    /// The rendered plan output.
    pub plan_output: PlanOutput,
}

/// One output group.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The group key values, in `group_by` order.
    pub key: Vec<ScalarValue>,
    /// The aggregate values, in the order the aggregates were declared.
    pub values: Vec<AggregateValue>,
}

/// The value of one aggregate within a group.
#[derive(Debug, Clone, PartialEq)]
pub enum AggregateValue {
    /// A floating-point aggregate (SUM of a float column).
    Float(f64),
    /// An integer aggregate (COUNT, COUNT(*)).
    Int(i64),
    /// A NULL aggregate (e.g. SUM over an all-NULL group).
    Null,
}

/// Where an aggregate's values come from.
///
/// The aggregate does not care which: it folds the values it is given, in the
/// order it is given them, and skips the NULLs among them. What differs is
/// where the values come from, and that is the difference between a column
/// that was materialised and a column that was read where it lies.
#[derive(Debug, Clone, Copy)]
pub enum ValueSource<'a> {
    /// The aggregate reads no column: it counts rows.
    Rows,
    /// A materialised column: the survivors, gathered into a column of their
    /// own.
    Gathered(&'a BatchColumn),
    /// The source column, read in place over the rows that survived.
    ///
    /// The column is never copied. The aggregate reads the survivors' values
    /// out of the source, one at a time, in the survivors' order.
    Selected {
        /// The column, read where it lies.
        column: SelectedColumn<'a>,
        /// The column's validity mask, over the source rows.
        validity: &'a NullBitmap,
        /// The rows that survived the filter.
        selection: &'a SelectionVector,
    },
}

/// A column read in place, over the rows that survived the filter.
///
/// The aggregate folds the column where it lies: it reads the survivors'
/// values out of the source and adds them, and no copy of them is ever made.
///
/// The column's **encoding** is what is read, so a dictionary-encoded column
/// is read through its dictionary -- the same values the gather would have
/// copied out of it, reached the same way.
#[derive(Debug, Clone, Copy)]
pub enum SelectedColumn<'a> {
    /// Plain doubles.
    Float64(&'a [f64]),
    /// Plain integers.
    Int64(&'a [i64]),
    /// Plain strings.
    Utf8(&'a [String]),
    /// Dictionary-encoded doubles.
    Float64Dict {
        /// The distinct values.
        dict: &'a [f64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded integers.
    Int64Dict {
        /// The distinct values.
        dict: &'a [i64],
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
}

impl<'a> SelectedColumn<'a> {
    /// The column in `chunk`, if its encoding can be read one row at a time.
    ///
    /// A run-length-encoded column cannot: its values are runs, and reading
    /// one row means finding the run it falls in. Such a column is gathered,
    /// as it always was.
    #[must_use]
    pub fn from_chunk(chunk: &'a ColumnChunk) -> Option<Self> {
        match &chunk.encoding {
            Encoding::Float64 { values } => Some(Self::Float64(values)),
            Encoding::Int64 { values } => Some(Self::Int64(values)),
            Encoding::Utf8 { values } => Some(Self::Utf8(values)),
            Encoding::Float64Dict { dict, indices } => Some(Self::Float64Dict { dict, indices }),
            Encoding::Int64Dict { dict, indices } => Some(Self::Int64Dict { dict, indices }),
            Encoding::Utf8Dict { dict, indices } => Some(Self::Utf8Dict { dict, indices }),
            Encoding::BoolRle { .. } => None,
        }
    }

    /// Whether the column's values are doubles.
    ///
    /// This is the same question the accumulator asks of a gathered column,
    /// and it is asked of the *encoding* rather than of a decoded column: a
    /// dictionary-encoded double column is a float column, exactly as its
    /// gathered copy is.
    #[must_use]
    pub const fn is_float(&self) -> bool {
        matches!(self, Self::Float64(_) | Self::Float64Dict { .. })
    }
}

impl ValueSource<'_> {
    /// Whether the values are doubles.
    ///
    /// This is the question the accumulator asks, and it is asked of the
    /// *source*: a dictionary-encoded double column is a float column, exactly
    /// as its gathered copy is.
    fn is_float(&self) -> bool {
        match self {
            Self::Rows => false,
            Self::Gathered(column) => matches!(column.data, BatchData::Float64(_)),
            Self::Selected { column, .. } => column.is_float(),
        }
    }
}

/// A column's values, read one row at a time.
///
/// The fold's inner loop asks for one row's value at a time, and this trait
/// is what the loop is written against. Resolving the column's **kind** once
/// -- here, in the type -- is what keeps the loop tight: the kind's test is
/// not in the loop, and the accumulator's state stays in a register.
///
/// The difference between a tight loop and a dispatching one is worth a
/// whole millisecond on the repository's own query, so the kind is resolved
/// before the loop rather than inside it.
trait RowValues {
    /// The value one row holds.
    type Value: Copy;
    /// The value of `row`.
    fn at(&self, row: usize) -> Self::Value;
}

/// A plain float column.
impl RowValues for &[f64] {
    type Value = f64;

    fn at(&self, row: usize) -> f64 {
        self[row]
    }
}

/// A plain integer column.
impl RowValues for &[i64] {
    type Value = i64;

    fn at(&self, row: usize) -> i64 {
        self[row]
    }
}

/// A dictionary-encoded float column.
struct DictF64<'a> {
    /// The distinct values.
    dict: &'a [f64],
    /// One index into the dictionary per row.
    indices: &'a [i32],
}

impl RowValues for DictF64<'_> {
    type Value = f64;

    fn at(&self, row: usize) -> f64 {
        let index = usize::try_from(self.indices[row]).expect("a dictionary index is not negative");
        self.dict[index]
    }
}

/// A dictionary-encoded integer column.
struct DictI64<'a> {
    /// The distinct values.
    dict: &'a [i64],
    /// One index into the dictionary per row.
    indices: &'a [i32],
}

impl RowValues for DictI64<'_> {
    type Value = i64;

    fn at(&self, row: usize) -> i64 {
        let index = usize::try_from(self.indices[row]).expect("a dictionary index is not negative");
        self.dict[index]
    }
}

/// A value the minimum and the maximum can be taken of.
///
/// Floats compare with `total_cmp`, so a NaN is **ordered** rather than
/// refused: the same rule the gathered column's fold uses, and the same rule
/// the aggregate's output order uses.
trait Orderable: Copy {
    /// Whether this value is smaller than `other`.
    fn is_less(&self, other: &Self) -> bool;
    /// Whether this value is larger than `other`.
    fn is_greater(&self, other: &Self) -> bool;
}

impl Orderable for f64 {
    fn is_less(&self, other: &Self) -> bool {
        self.total_cmp(other).is_lt()
    }

    fn is_greater(&self, other: &Self) -> bool {
        self.total_cmp(other).is_gt()
    }
}

impl Orderable for i64 {
    fn is_less(&self, other: &Self) -> bool {
        self < other
    }

    fn is_greater(&self, other: &Self) -> bool {
        self > other
    }
}

/// Which way an extreme goes.
#[derive(Debug, Clone, Copy)]
enum Extreme {
    /// The smallest value.
    Min,
    /// The largest value.
    Max,
}

/// Folds the survivors' values into a running float sum.
///
/// The sum is taken in the survivors' **ascending order**, so it is bit for
/// bit the sum the gathered column's fold took.
fn fold_sum_f64<R: RowValues<Value = f64>>(
    total: &mut f64,
    seen: &mut bool,
    rows: &R,
    validity: &NullBitmap,
    selection: &SelectionVector,
) {
    let mut sum = *total;
    let mut any = *seen;
    selection.for_each(|row| {
        let row = usize::try_from(row).expect("a row fits in a usize");
        if row < validity.len() && validity.is_valid(row) {
            sum += rows.at(row);
            any = true;
        }
    });
    *total = sum;
    *seen = any;
}

/// Folds the survivors' values into a running integer sum.
///
/// The sum **saturates** at the `i64` bounds rather than wrapping, so an
/// overflowing sum is loud rather than wrong.
fn fold_sum_i64<R: RowValues<Value = i64>>(
    total: &mut i64,
    seen: &mut bool,
    rows: &R,
    validity: &NullBitmap,
    selection: &SelectionVector,
) {
    let mut sum = *total;
    let mut any = *seen;
    selection.for_each(|row| {
        let row = usize::try_from(row).expect("a row fits in a usize");
        if row < validity.len() && validity.is_valid(row) {
            sum = sum.saturating_add(rows.at(row));
            any = true;
        }
    });
    *total = sum;
    *seen = any;
}

/// Folds the survivors' values into a running minimum or maximum.
fn fold_extreme<R: RowValues>(
    current: &mut Option<R::Value>,
    extreme: Extreme,
    rows: &R,
    validity: &NullBitmap,
    selection: &SelectionVector,
) where
    R::Value: Orderable,
{
    let mut best = *current;
    selection.for_each(|row| {
        let row = usize::try_from(row).expect("a row fits in a usize");
        if row < validity.len() && validity.is_valid(row) {
            let value = rows.at(row);
            let better = match best {
                None => true,
                Some(best) => match extreme {
                    Extreme::Min => value.is_less(&best),
                    Extreme::Max => value.is_greater(&best),
                },
            };
            if better {
                best = Some(value);
            }
        }
    });
    *current = best;
}

/// Folds the survivors' non-NULL rows into a count.
///
/// A count reads **no values at all**: it only asks which rows hold one.
/// The gathered column's fold had to read the values to find out, because a
/// gathered column carries its NULLs in a mask the count had to walk.
fn fold_count(count: &mut i64, validity: &NullBitmap, selection: &SelectionVector) {
    let mut counted = *count;
    selection.for_each(|row| {
        let row = usize::try_from(row).expect("a row fits in a usize");
        if row < validity.len() && validity.is_valid(row) {
            counted += 1;
        }
    });
    *count = counted;
}

/// Computes the hash aggregate.
///
/// `group_by` holds the gathered group-by columns, in `group_by` order.
/// `values` holds the gathered value column of each aggregate, in the order
/// the aggregates were declared; `None` marks `COUNT(*)`, which reads no
/// column. `rows` is the number of rows the aggregation covers.
///
/// This is the **grouped** path's entry point. A global aggregation -- one
/// with no GROUP BY -- is better served by [`aggregate_global`], which can
/// read the columns where they lie instead of gathering them.
#[must_use]
pub fn aggregate<'a>(
    spec: &AggregateSpec,
    group_by: &[&'a BatchColumn],
    values: &[Option<&'a BatchColumn>],
    rows: usize,
) -> Vec<Group> {
    // A global aggregate -- one with no GROUP BY -- has exactly one group, and
    // it is the same group for every row. Running the hash machinery over it
    // would build a key vector and do a hash lookup per row to arrive at the
    // same answer every time, which is why the first version of this
    // operator spent 48 ms counting six million rows.
    //
    // The global fold itself is [`global_group`], and it is the *same* fold
    // [`aggregate_global`] runs: one implementation, so there is no second
    // answer to disagree with this one.
    if group_by.is_empty() {
        let sources: Vec<ValueSource<'a>> = values
            .iter()
            .map(|value| match value {
                None => ValueSource::Rows,
                Some(column) => ValueSource::Gathered(column),
            })
            .collect();
        return vec![global_group(spec, &sources, rows)];
    }

    let mut slots: HashMap<Vec<KeyPart>, usize> = HashMap::new();
    let mut states: Vec<GroupState> = Vec::new();

    for row in 0..rows {
        // A row with a NULL in its group key belongs to no group.
        let Some(key) = key_of(group_by, row) else {
            continue;
        };

        let slot = if let Some(&slot) = slots.get(&key) {
            slot
        } else {
            let slot = states.len();
            slots.insert(key.clone(), slot);
            states.push(GroupState::new(&key, spec, values));
            slot
        };

        if let Some(state) = states.get_mut(slot) {
            state.accumulate(row, values);
        }
    }

    // The output order is the sorted key order, not the hash order.
    states.sort_by(|left, right| left.key.cmp(&right.key));

    states
        .into_iter()
        .map(|state| Group {
            key: state.key.iter().map(key_to_scalar).collect(),
            values: state.accumulators.iter().map(Accumulator::finish).collect(),
        })
        .collect()
}

/// Computes the single group of a global aggregate.
///
/// This is the global path's entry point, and it is the one that can read a
/// column where it lies: a [`ValueSource::Selected`] column is folded over
/// the selection, and no column of the survivors is ever built.
///
/// `values` holds the source of each aggregate's values, in the order the
/// aggregates were declared. `rows` is the number of rows the aggregation
/// covers.
#[must_use]
pub fn aggregate_global(
    spec: &AggregateSpec,
    values: &[ValueSource<'_>],
    rows: usize,
) -> Vec<Group> {
    vec![global_group(spec, values, rows)]
}

/// Computes the single group of a global aggregate.
///
/// Each aggregate is folded on its own, so that `COUNT(*)` never reads a
/// column and never touches the row loop at all.
fn global_group(spec: &AggregateSpec, values: &[ValueSource<'_>], rows: usize) -> Group {
    let mut accumulators: Vec<Accumulator> = spec
        .aggregates
        .iter()
        .zip(values.iter())
        .map(|(aggregate, source)| Accumulator::new(aggregate, source.is_float()))
        .collect();

    for (index, (aggregate, source)) in spec.aggregates.iter().zip(values.iter()).enumerate() {
        match (aggregate, source) {
            // `COUNT(*)` counts rows, not values. The answer is the row
            // count, so no column is read and no row is visited.
            (Aggregate::CountStar, _) => accumulators[index].set_row_count(rows),
            // The column is read where it lies: the survivors' values are
            // folded straight out of the source, and no column of the
            // survivors is ever built.
            (_, ValueSource::Selected { column, validity, selection }) => {
                accumulators[index].fold_selected(column, validity, selection);
            }
            (_, ValueSource::Gathered(column)) => {
                for row in 0..rows {
                    if let Some(value) = raw_at(column, row) {
                        accumulators[index].add(value);
                    }
                }
            }
            (_, ValueSource::Rows) => {
                for _ in 0..rows {
                    accumulators[index].add(RawValue::Unit);
                }
            }
        }
    }

    Group { key: Vec::new(), values: accumulators.iter().map(Accumulator::finish).collect() }
}

/// The state of one group while the aggregation runs.
struct GroupState {
    /// The group's key.
    key: Vec<KeyPart>,
    /// One accumulator per declared aggregate.
    accumulators: Vec<Accumulator>,
}

impl GroupState {
    /// Builds a group's accumulators from the plan's aggregates.
    fn new(key: &[KeyPart], spec: &AggregateSpec, values: &[Option<&BatchColumn>]) -> Self {
        let accumulators = spec
            .aggregates
            .iter()
            .zip(values.iter())
            .map(|(aggregate, column)| {
                let is_float =
                    column.is_some_and(|column| matches!(column.data, BatchData::Float64(_)));
                Accumulator::new(aggregate, is_float)
            })
            .collect();
        Self { key: key.to_vec(), accumulators }
    }

    /// Folds one row into the group's accumulators.
    fn accumulate(&mut self, row: usize, values: &[Option<&BatchColumn>]) {
        for (accumulator, column) in self.accumulators.iter_mut().zip(values.iter()) {
            match column {
                Some(column) => {
                    if let Some(value) = raw_at(column, row) {
                        accumulator.add(value);
                    }
                }
                // `COUNT(*)` counts the row itself; it reads no column.
                None => accumulator.add(RawValue::Unit),
            }
        }
    }
}

/// One aggregate's running state.
enum Accumulator {
    /// A running integer sum.
    SumInt {
        /// The sum so far.
        total: i64,
        /// Whether any value was folded in.
        seen: bool,
    },
    /// A running floating-point sum.
    SumFloat {
        /// The sum so far.
        total: f64,
        /// Whether any value was folded in.
        seen: bool,
    },
    /// A count of non-NULL values.
    Count {
        /// The count so far.
        count: i64,
    },
    /// A count of rows.
    CountStar {
        /// The count so far.
        count: i64,
    },
    /// The smallest value seen.
    MinInt {
        /// The smallest value so far.
        value: Option<i64>,
    },
    /// The largest value seen.
    MaxInt {
        /// The largest value so far.
        value: Option<i64>,
    },
    /// The smallest value seen.
    MinFloat {
        /// The smallest value so far.
        value: Option<f64>,
    },
    /// The largest value seen.
    MaxFloat {
        /// The largest value so far.
        value: Option<f64>,
    },
}

impl Accumulator {
    /// Builds the accumulator an aggregate needs.
    ///
    /// The accumulator's type comes from the column's type, so the type
    /// check the plan resolution made is carried through to the fold.
    fn new(aggregate: &Aggregate, is_float: bool) -> Self {
        match aggregate {
            Aggregate::Sum { .. } => {
                if is_float {
                    Self::SumFloat { total: 0.0, seen: false }
                } else {
                    Self::SumInt { total: 0, seen: false }
                }
            }
            Aggregate::Count { .. } => Self::Count { count: 0 },
            Aggregate::CountStar => Self::CountStar { count: 0 },
            Aggregate::Min { .. } => {
                if is_float {
                    Self::MinFloat { value: None }
                } else {
                    Self::MinInt { value: None }
                }
            }
            Aggregate::Max { .. } => {
                if is_float {
                    Self::MaxFloat { value: None }
                } else {
                    Self::MaxInt { value: None }
                }
            }
        }
    }

    /// Folds one value in.
    fn add(&mut self, value: RawValue<'_>) {
        match (self, value) {
            (Self::SumInt { total, seen }, RawValue::Int(value)) => {
                *total = total.saturating_add(value);
                *seen = true;
            }
            (Self::SumFloat { total, seen }, RawValue::Float(value)) => {
                *total += value;
                *seen = true;
            }
            (Self::Count { count }, RawValue::Int(_) | RawValue::Float(_) | RawValue::Utf8(_))
            | (Self::CountStar { count }, RawValue::Unit) => {
                *count += 1;
            }
            (Self::MinInt { value: current }, RawValue::Int(value)) => {
                if current.is_none_or(|current| value < current) {
                    *current = Some(value);
                }
            }
            (Self::MaxInt { value: current }, RawValue::Int(value)) => {
                if current.is_none_or(|current| value > current) {
                    *current = Some(value);
                }
            }
            (Self::MinFloat { value: current }, RawValue::Float(value)) => {
                if current.is_none_or(|current| value.total_cmp(&current).is_lt()) {
                    *current = Some(value);
                }
            }
            (Self::MaxFloat { value: current }, RawValue::Float(value)) => {
                if current.is_none_or(|current| value.total_cmp(&current).is_gt()) {
                    *current = Some(value);
                }
            }
            // The accumulator's type is chosen from the column's type, so a
            // value of another type cannot reach here.
            (_, _) => {}
        }
    }

    /// Folds a column read in place over the rows that survived.
    ///
    /// The survivors are visited in **ascending order** -- the order the
    /// gathered column's fold visits them in -- so a float sum is bit for bit
    /// the sum the gathered column produced. A NULL row is skipped, exactly
    /// as `raw_at` skips it, and so is a row beyond the validity mask's
    /// length: a row the engine cannot vouch for is a NULL row.
    ///
    /// The accumulator's type and the column's type agree -- the accumulator
    /// was built from the column's type -- so the match below has one arm per
    /// **pair** that can occur. Each arm is a tight loop: the column's kind
    /// and the accumulator's state are both resolved before the loop starts.
    fn fold_selected(
        &mut self,
        column: &SelectedColumn<'_>,
        validity: &NullBitmap,
        selection: &SelectionVector,
    ) {
        match (&mut *self, column) {
            (Self::SumFloat { total, seen }, SelectedColumn::Float64(values)) => {
                fold_sum_f64(total, seen, values, validity, selection);
            }
            (Self::SumFloat { total, seen }, SelectedColumn::Float64Dict { dict, indices }) => {
                fold_sum_f64(total, seen, &DictF64 { dict, indices }, validity, selection);
            }
            (Self::SumInt { total, seen }, SelectedColumn::Int64(values)) => {
                fold_sum_i64(total, seen, values, validity, selection);
            }
            (Self::SumInt { total, seen }, SelectedColumn::Int64Dict { dict, indices }) => {
                fold_sum_i64(total, seen, &DictI64 { dict, indices }, validity, selection);
            }
            // A count reads no values: it only asks which rows hold one.
            (Self::Count { count }, _) => fold_count(count, validity, selection),
            (Self::MinInt { value }, SelectedColumn::Int64(values)) => {
                fold_extreme(value, Extreme::Min, values, validity, selection);
            }
            (Self::MinInt { value }, SelectedColumn::Int64Dict { dict, indices }) => {
                fold_extreme(value, Extreme::Min, &DictI64 { dict, indices }, validity, selection);
            }
            (Self::MaxInt { value }, SelectedColumn::Int64(values)) => {
                fold_extreme(value, Extreme::Max, values, validity, selection);
            }
            (Self::MaxInt { value }, SelectedColumn::Int64Dict { dict, indices }) => {
                fold_extreme(value, Extreme::Max, &DictI64 { dict, indices }, validity, selection);
            }
            (Self::MinFloat { value }, SelectedColumn::Float64(values)) => {
                fold_extreme(value, Extreme::Min, values, validity, selection);
            }
            (Self::MinFloat { value }, SelectedColumn::Float64Dict { dict, indices }) => {
                fold_extreme(value, Extreme::Min, &DictF64 { dict, indices }, validity, selection);
            }
            (Self::MaxFloat { value }, SelectedColumn::Float64(values)) => {
                fold_extreme(value, Extreme::Max, values, validity, selection);
            }
            (Self::MaxFloat { value }, SelectedColumn::Float64Dict { dict, indices }) => {
                fold_extreme(value, Extreme::Max, &DictF64 { dict, indices }, validity, selection);
            }
            // The accumulator's type is chosen from the column's type, so a
            // value of another type cannot reach here -- and a sum over a
            // string column is refused by the plan before it runs.
            (_, _) => {}
        }
    }

    /// Sets a `COUNT(*)` accumulator to the number of rows it covers.
    ///
    /// `COUNT(*)` counts rows rather than values, so the whole row loop can be
    /// skipped for it. Calling this on any other accumulator would be a bug,
    /// so it is asserted rather than ignored.
    fn set_row_count(&mut self, rows: usize) {
        match self {
            Self::CountStar { count } => *count = i64::try_from(rows).unwrap_or(i64::MAX),
            _ => debug_assert!(
                false,
                "set_row_count was called on an accumulator that is not COUNT(*)"
            ),
        }
    }

    /// The aggregate's final value.
    fn finish(&self) -> AggregateValue {
        match self {
            Self::SumInt { total, seen } => {
                if *seen {
                    AggregateValue::Int(*total)
                } else {
                    AggregateValue::Null
                }
            }
            Self::SumFloat { total, seen } => {
                if *seen {
                    AggregateValue::Float(*total)
                } else {
                    AggregateValue::Null
                }
            }
            Self::Count { count } | Self::CountStar { count } => AggregateValue::Int(*count),
            Self::MinInt { value } | Self::MaxInt { value } => match value {
                Some(value) => AggregateValue::Int(*value),
                None => AggregateValue::Null,
            },
            Self::MinFloat { value } | Self::MaxFloat { value } => match value {
                Some(value) => AggregateValue::Float(*value),
                None => AggregateValue::Null,
            },
        }
    }
}

/// A raw column value, borrowed.
enum RawValue<'a> {
    /// An integer.
    Int(i64),
    /// A double.
    Float(f64),
    /// A string.
    Utf8(&'a str),
    /// The absence of a value: `COUNT(*)` counts the row itself.
    Unit,
}

/// Reads the value of `row` out of a gathered column.
///
/// Returns `None` when the row holds NULL.
fn raw_at(column: &BatchColumn, row: usize) -> Option<RawValue<'_>> {
    if row >= column.validity.len() || !column.validity.is_valid(row) {
        return None;
    }
    match &column.data {
        BatchData::Int64(values) => {
            Some(RawValue::Int(*values.get(row).expect("the row is inside the column")))
        }
        BatchData::Float64(values) => {
            Some(RawValue::Float(*values.get(row).expect("the row is inside the column")))
        }
        BatchData::Utf8(values) => {
            Some(RawValue::Utf8(values.get(row).expect("the row is inside the column").as_str()))
        }
    }
}

/// One part of a group key.
///
/// The key is hashed on the *bit pattern* of a float, never on the float
/// itself: `f64` is not `Eq`, and hashing a float would make the hash map's
/// behaviour depend on how NaN compares to itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum KeyPart {
    /// An integer key.
    Int(i64),
    /// A float key, held as its bit pattern.
    Float(u64),
    /// A string key.
    Utf8(String),
}

impl KeyPart {
    /// The variant's rank, used to order keys of different types.
    const fn rank(&self) -> u8 {
        match self {
            Self::Int(_) => 0,
            Self::Float(_) => 1,
            Self::Utf8(_) => 2,
        }
    }
}

impl Ord for KeyPart {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Int(left), Self::Int(right)) => left.cmp(right),
            (Self::Float(left), Self::Float(right)) => {
                f64::from_bits(*left).total_cmp(&f64::from_bits(*right))
            }
            (Self::Utf8(left), Self::Utf8(right)) => left.cmp(right),
            (left, right) => left.rank().cmp(&right.rank()),
        }
    }
}

impl PartialOrd for KeyPart {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The bit pattern a float is keyed on.
///
/// `-0.0` is normalised to `+0.0`, so the two group together: they are the
/// same number, and a group key that split them would be surprising. NaN
/// keeps its bit pattern, so a NaN key groups with the NaNs that share it
/// and with nothing else.
fn float_key(value: f64) -> u64 {
    let bits = value.to_bits();
    if bits == (-0.0f64).to_bits() {
        0.0f64.to_bits()
    } else {
        bits
    }
}

/// The group key of `row`, or `None` when the key holds a NULL.
fn key_of(group_by: &[&BatchColumn], row: usize) -> Option<Vec<KeyPart>> {
    let mut key = Vec::with_capacity(group_by.len());
    for column in group_by {
        let part = match raw_at(column, row)? {
            RawValue::Int(value) => KeyPart::Int(value),
            RawValue::Float(value) => KeyPart::Float(float_key(value)),
            RawValue::Utf8(value) => KeyPart::Utf8(value.to_owned()),
            RawValue::Unit => return None,
        };
        key.push(part);
    }
    Some(key)
}

/// The scalar value of a key part.
fn key_to_scalar(part: &KeyPart) -> ScalarValue {
    match part {
        KeyPart::Int(value) => ScalarValue::Int64(*value),
        KeyPart::Float(bits) => ScalarValue::Float64(f64::from_bits(*bits)),
        KeyPart::Utf8(value) => ScalarValue::Utf8(value.clone()),
    }
}
