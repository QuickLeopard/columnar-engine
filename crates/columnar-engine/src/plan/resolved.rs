//! The plan the executor reads.
//!
//! The same plan as the caller wrote it, with every column name resolved to
//! an index and every type checked. Nothing here knows how a plan is
//! written; it knows only what one means once it has been checked.

use columnar_core::{ArithOp, ComparisonOp, DataType, Predicate, ScalarValue};

use crate::EngineError;

use super::spec::{AggregateSpec, JoinSpec, SortSpec};

/// The mapping from a schema's columns onto a subset of them.
///
/// The plan is resolved against the **file's** schema, and the dataset it
/// ends up running over holds only the columns the read set asked for. This
/// is how a plan's indices move onto that subset.
///
/// It is built from the read set, so it holds every column the read set
/// holds. A column that is not in it is a column the dataset does not have,
/// and a plan that reads one is an **error the caller sees** -- not a crash
/// inside the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnMapping {
    /// One entry per column of the schema: the column's index in the subset,
    /// or `None` when the column is not in it.
    entries: Vec<Option<usize>>,
}

impl ColumnMapping {
    /// The mapping for a schema of `columns` columns, of which `read` are in
    /// the subset.
    ///
    /// `read` must be **sorted and without duplicates**: the mapping is
    /// built by looking each column up in it.
    #[must_use]
    pub fn of(read: &[usize], columns: usize) -> Self {
        Self { entries: (0..columns).map(|index| read.binary_search(&index).ok()).collect() }
    }

    /// The index of `column` in the subset, when it is in it.
    #[must_use]
    pub fn get(&self, column: usize) -> Option<usize> {
        self.entries.get(column).copied().flatten()
    }

    /// The index of `column` in the subset.
    ///
    /// # Errors
    ///
    /// The column is not in the subset: the plan reads a column the dataset
    /// does not have. That is a plan error -- the plan was resolved against
    /// one schema and run over another, and the two disagree.
    pub fn require(&self, column: usize) -> Result<usize, EngineError> {
        self.get(column).ok_or_else(|| {
            EngineError::InvalidPlan(format!(
                "the plan reads column {column}, and the dataset does not hold it"
            ))
        })
    }
}

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
    fn remap(&self, mapping: &ColumnMapping) -> Result<ResolvedExpr, EngineError> {
        Ok(match self {
            Self::Constant(value) => Self::Constant(value.clone()),
            Self::Column { index, data_type } => {
                Self::Column { index: mapping.require(*index)?, data_type: *data_type }
            }
            Self::Binary { op, left, right } => Self::Binary {
                op: *op,
                left: Box::new(left.remap(mapping)?),
                right: Box::new(right.remap(mapping)?),
            },
            Self::Negate(inner) => Self::Negate(Box::new(inner.remap(mapping)?)),
        })
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
    /// The probe side's key column's type.
    ///
    /// The type is resolved **with** the index, out of the same schema, so
    /// that the join's two keys can be compared without reading either schema
    /// again -- and so that both paths compare the same thing. A remap moves
    /// the index and keeps the type: the column is the same column.
    pub probe_type: DataType,
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
    ///
    /// # Errors
    ///
    /// The plan reads a column the dataset does not hold.
    pub fn remap(&self, mapping: &ColumnMapping) -> Result<ResolvedPlan, EngineError> {
        let filter: Vec<ResolvedFilter> = self
            .filter
            .iter()
            .filter_map(|filter| {
                let column = mapping.get(filter.column)?;
                Some(ResolvedFilter { column, predicate: filter.predicate.clone() })
            })
            .collect();

        let projection: Vec<usize> =
            self.projection.iter().filter_map(|&index| mapping.get(index)).collect();

        let aggregate =
            self.aggregate.as_ref().map(|aggregate| aggregate.remap(mapping)).transpose()?;

        let join = self
            .join
            .as_ref()
            .map(|join| {
                Ok::<ResolvedJoin, EngineError>(ResolvedJoin {
                    spec: join.spec.clone(),
                    probe_key: mapping.require(join.probe_key)?,
                    probe_type: join.probe_type,
                })
            })
            .transpose()?;

        let sort = self
            .sort
            .as_ref()
            .map(|sort| {
                Ok::<ResolvedSort, EngineError>(ResolvedSort {
                    spec: sort.spec.clone(),
                    column: mapping.require(sort.column)?,
                })
            })
            .transpose()?;

        let mut materialised = Vec::with_capacity(self.materialised.len());
        for &index in &self.materialised {
            materialised.push(mapping.require(index)?);
        }

        Ok(ResolvedPlan { filter, projection, aggregate, join, sort, materialised })
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
    fn remap(&self, mapping: &ColumnMapping) -> Result<ResolvedAggregate, EngineError> {
        let mut group_by = Vec::with_capacity(self.group_by.len());
        for &index in &self.group_by {
            group_by.push(mapping.require(index)?);
        }
        let mut aggregates = Vec::with_capacity(self.aggregates.len());
        for function in &self.aggregates {
            aggregates.push(function.remap(mapping)?);
        }
        Ok(ResolvedAggregate { spec: self.spec.clone(), group_by, aggregates })
    }
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
    fn remap(&self, mapping: &ColumnMapping) -> Result<ResolvedAggregateFn, EngineError> {
        Ok(match self {
            Self::Sum { expr } => Self::Sum { expr: expr.remap(mapping)? },
            Self::Count { column } => Self::Count { column: mapping.require(*column)? },
            Self::Min { column } => Self::Min { column: mapping.require(*column)? },
            Self::Max { column } => Self::Max { column: mapping.require(*column)? },
            Self::CountStar => Self::CountStar,
        })
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
