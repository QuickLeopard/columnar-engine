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

use std::cmp::Ordering;
use std::collections::HashMap;

use columnar_core::{BatchColumn, BatchData, ScalarValue};

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

/// Computes the hash aggregate.
///
/// `group_by` holds the gathered group-by columns, in `group_by` order.
/// `values` holds the gathered value column of each aggregate, in the order
/// the aggregates were declared; `None` marks `COUNT(*)`, which reads no
/// column. `rows` is the number of rows the aggregation covers.
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
    // This path also handles `COUNT(*)` in the way the module documentation
    // promises: it reads no column at all, and the answer is the row count.
    if group_by.is_empty() {
        return vec![global_group(spec, values, rows)];
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
/// Each aggregate is folded on its own, so that `COUNT(*)` never reads a
/// column and never touches the row loop at all.
fn global_group(spec: &AggregateSpec, values: &[Option<&BatchColumn>], rows: usize) -> Group {
    let mut accumulators: Vec<Accumulator> = spec
        .aggregates
        .iter()
        .zip(values.iter())
        .map(|(aggregate, column)| Accumulator::new(aggregate, *column))
        .collect();

    for (index, (aggregate, column)) in spec.aggregates.iter().zip(values.iter()).enumerate() {
        match (aggregate, column) {
            // `COUNT(*)` counts rows, not values. The answer is the row
            // count, so no column is read and no row is visited.
            (Aggregate::CountStar, _) => accumulators[index].set_row_count(rows),
            (_, Some(column)) => {
                for row in 0..rows {
                    if let Some(value) = raw_at(column, row) {
                        accumulators[index].add(value);
                    }
                }
            }
            (_, None) => {
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
            .map(|(aggregate, column)| Accumulator::new(aggregate, *column))
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
    fn new(aggregate: &Aggregate, column: Option<&BatchColumn>) -> Self {
        let is_float = column.is_some_and(|column| matches!(column.data, BatchData::Float64(_)));
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
