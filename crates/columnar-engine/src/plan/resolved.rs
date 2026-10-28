//! The plan the executor reads.
//!
//! The same plan as the caller wrote it, with every column name resolved to
//! an index and every type checked. Nothing here knows how a plan is
//! written; it knows only what one means once it has been checked.

use columnar_core::{ArithOp, ComparisonOp, DataType, Predicate, ScalarValue};

use super::spec::{AggregateSpec, JoinSpec, SortSpec};

/// An expression whose column names have been resolved to indices.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedExpr {
    /// A literal value.
    Constant(ScalarValue),
    /// A reference to a column, by index.
    Column {
        /// The column's index.
        index: usize,
        /// The column's type.
        ///
        /// The type is carried here rather than looked up again, because the
        /// expression's type has to be known to evaluate it, and the schema
        /// the expression was resolved against is not available at that point.
        data_type: DataType,
    },
    /// An arithmetic operation over two expressions.
    Binary {
        /// The operator.
        op: ArithOp,
        /// The left operand.
        left: Box<ResolvedExpr>,
        /// The right operand.
        right: Box<ResolvedExpr>,
    },
    /// The negation of an expression.
    Negate(Box<ResolvedExpr>),
}

impl ResolvedExpr {
    /// The expression's type.
    ///
    /// `int64` only when every operand is `int64`, and `float64` otherwise.
    #[must_use]
    pub fn data_type(&self) -> DataType {
        match self {
            Self::Constant(value) => value.data_type(),
            Self::Column { data_type, .. } => *data_type,
            Self::Binary { left, right, .. } => {
                if left.data_type() == DataType::Float64 || right.data_type() == DataType::Float64 {
                    DataType::Float64
                } else {
                    DataType::Int64
                }
            }
            Self::Negate(inner) => inner.data_type(),
        }
    }

    /// The columns the expression reads.
    #[must_use]
    pub fn columns(&self) -> Vec<usize> {
        let mut columns: Vec<usize> = Vec::new();
        self.collect_columns(&mut columns);
        columns
    }

    /// Collects the columns the expression reads.
    fn collect_columns(&self, columns: &mut Vec<usize>) {
        match self {
            Self::Constant(_) => {}
            Self::Column { index, .. } => {
                if !columns.contains(index) {
                    columns.push(*index);
                }
            }
            Self::Binary { left, right, .. } => {
                left.collect_columns(columns);
                right.collect_columns(columns);
            }
            Self::Negate(inner) => inner.collect_columns(columns),
        }
    }

    /// Maps this expression's column indices onto a subset of the columns.
    fn remap(&self, mapping: &[Option<usize>]) -> ResolvedExpr {
        match self {
            Self::Constant(value) => Self::Constant(value.clone()),
            Self::Column { index, data_type } => {
                Self::Column { index: remap_index(mapping, *index), data_type: *data_type }
            }
            Self::Binary { op, left, right } => Self::Binary {
                op: *op,
                left: Box::new(left.remap(mapping)),
                right: Box::new(right.remap(mapping)),
            },
            Self::Negate(inner) => Self::Negate(Box::new(inner.remap(mapping))),
        }
    }
}

/// A plan whose column names have been resolved to indices.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPlan {
    /// The filters, with their columns resolved. Every one must hold.
    pub filter: Vec<ResolvedFilter>,
    /// The projection's column indices. Empty means "all columns".
    pub projection: Vec<usize>,
    /// The aggregation, with its columns resolved.
    pub aggregate: Option<ResolvedAggregate>,
    /// The join, with its probe key resolved.
    ///
    /// The **build** side is resolved when the join reads it: it is a file
    /// of its own, with a schema of its own, and the plan does not hold it.
    pub join: Option<ResolvedJoin>,
    /// The sort, with its column resolved.
    pub sort: Option<ResolvedSort>,
    /// The columns the plan will decode, in schema order.
    pub materialised: Vec<usize>,
}

/// A join whose probe-side column has been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedJoin {
    /// The join as the caller wrote it.
    pub spec: JoinSpec,
    /// The probe side's key column, by index.
    pub probe_key: usize,
}

/// A sort whose column has been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSort {
    /// The sort as the caller wrote it.
    pub spec: SortSpec,
    /// The column the rows are sorted by, by index.
    pub column: usize,
}

impl ResolvedPlan {
    /// Maps this plan's column indices onto a subset of the columns.
    ///
    /// `mapping` has one entry per column of the schema the plan was
    /// resolved against: the entry is the column's index in the subset, or
    /// `None` when the column is not in it.
    ///
    /// The lazy path needs this. The plan is resolved against the *file's*
    /// schema, and the dataset it ends up running over holds only the
    /// columns the read set asked for, so every index has to move.
    ///
    /// **The filters whose column is not in the subset are dropped.** That is
    /// not a silent omission: a filter is only left out of the subset when
    /// it was pushed into the Parquet reader, which already applied it. The
    /// conjunction is separable, so `(A ∧ B)` applied by the reader and then
    /// `B` applied by the engine is `A ∧ B`.
    #[must_use]
    pub fn remap(&self, mapping: &[Option<usize>]) -> ResolvedPlan {
        let filter = self
            .filter
            .iter()
            .filter_map(|filter| {
                let column = mapping.get(filter.column).copied().flatten()?;
                Some(ResolvedFilter { column, predicate: filter.predicate.clone() })
            })
            .collect();

        let projection = self
            .projection
            .iter()
            .filter_map(|&index| mapping.get(index).copied().flatten())
            .collect();

        let aggregate = self.aggregate.as_ref().map(|aggregate| aggregate.remap(mapping));

        let join = self.join.as_ref().map(|join| ResolvedJoin {
            spec: join.spec.clone(),
            probe_key: remap_index(mapping, join.probe_key),
        });

        let sort = self.sort.as_ref().map(|sort| ResolvedSort {
            spec: sort.spec.clone(),
            column: remap_index(mapping, sort.column),
        });

        let materialised = self
            .materialised
            .iter()
            .filter_map(|&index| mapping.get(index).copied().flatten())
            .collect();

        ResolvedPlan { filter, projection, aggregate, join, sort, materialised }
    }
}

/// A filter whose column has been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFilter {
    /// The index of the filtered column.
    pub column: usize,
    /// The predicate itself.
    pub predicate: Predicate,
}

/// An aggregation whose columns have been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAggregate {
    /// The aggregation as the caller wrote it.
    pub spec: AggregateSpec,
    /// The indices of the group-by columns, in declaration order.
    pub group_by: Vec<usize>,
    /// The aggregates, with their columns resolved.
    pub aggregates: Vec<ResolvedAggregateFn>,
}

impl ResolvedAggregate {
    /// The columns the aggregation reads.
    ///
    /// The group-by columns, and the columns every aggregate function reads.
    /// An expression's columns are among them: the expression is evaluated
    /// over the rows the aggregation covers, so its columns have to be
    /// materialised too.
    #[must_use]
    pub fn columns(&self) -> Vec<usize> {
        let mut columns: Vec<usize> = Vec::new();
        let mut push = |column: usize| {
            if !columns.contains(&column) {
                columns.push(column);
            }
        };
        for &column in &self.group_by {
            push(column);
        }
        for function in &self.aggregates {
            for column in function.columns() {
                push(column);
            }
        }
        columns
    }

    /// Maps this aggregation's column indices onto a subset of the columns.
    ///
    /// Every column an aggregation reads is materialised, so every one of
    /// them is in the subset; a column that is not would mean the aggregate
    /// was reading something nobody read.
    fn remap(&self, mapping: &[Option<usize>]) -> ResolvedAggregate {
        let group_by = self.group_by.iter().map(|&index| remap_index(mapping, index)).collect();
        let aggregates = self.aggregates.iter().map(|function| function.remap(mapping)).collect();
        ResolvedAggregate { spec: self.spec.clone(), group_by, aggregates }
    }
}

/// Maps one column index onto a subset.
fn remap_index(mapping: &[Option<usize>], index: usize) -> usize {
    mapping.get(index).copied().flatten().expect("a materialised column is in the read set")
}

/// One resolved aggregate function.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedAggregateFn {
    /// SUM of an expression.
    Sum {
        /// The expression to sum, with its columns resolved.
        expr: ResolvedExpr,
    },
    /// COUNT of a column's non-NULL values.
    Count {
        /// The index of the counted column.
        column: usize,
    },
    /// COUNT(*) over the group.
    CountStar,
    /// MIN of a column.
    Min {
        /// The index of the column.
        column: usize,
    },
    /// MAX of a column.
    Max {
        /// The index of the column.
        column: usize,
    },
}

impl ResolvedAggregateFn {
    /// The columns the aggregate reads.
    ///
    /// A `SUM` over an expression reads every column the expression reads;
    /// the other functions read exactly one.
    #[must_use]
    pub fn columns(&self) -> Vec<usize> {
        match self {
            Self::Sum { expr } => expr.columns(),
            Self::Count { column } | Self::Min { column } | Self::Max { column } => vec![*column],
            Self::CountStar => Vec::new(),
        }
    }

    /// Maps this function's column indices onto a subset of the columns.
    fn remap(&self, mapping: &[Option<usize>]) -> ResolvedAggregateFn {
        match self {
            Self::Sum { expr } => Self::Sum { expr: expr.remap(mapping) },
            Self::Count { column } => Self::Count { column: remap_index(mapping, *column) },
            Self::Min { column } => Self::Min { column: remap_index(mapping, *column) },
            Self::Max { column } => Self::Max { column: remap_index(mapping, *column) },
            Self::CountStar => Self::CountStar,
        }
    }
}

/// The value a predicate compares against, as a typed threshold.
impl ResolvedFilter {
    /// The comparison operator.
    #[must_use]
    pub const fn op(&self) -> ComparisonOp {
        self.predicate.op
    }

    /// The threshold the column is compared against.
    #[must_use]
    pub fn value(&self) -> &ScalarValue {
        &self.predicate.value
    }
}
