//! The typed query plan and its resolution.
//!
//! There is no SQL parser. The query surface is the [`QueryPlan`] struct,
//! built in Rust by the caller, and the plan output prints it back so a
//! reader can see exactly what ran.
//!
//! ## The pinned optimisation order
//!
//! The engine applies its optimisations in exactly this order, and no other:
//!
//! 1. **Predicate pushdown** -- the filter is evaluated inside the scan, so
//!    the rows that fail it never reach the operators behind it.
//! 2. **Projection pushdown** -- only the columns the plan reads are
//!    considered at all.
//! 3. **Late materialisation** -- of those columns, only the ones the plan
//!    actually reads are decoded. A column that is neither filtered nor
//!    aggregated is never touched, and the plan output says so by name.
//! 4. **Aggregate** -- the aggregation runs last, over the surviving rows.
//!
//! The order is a constant, [`OPTIMIZATION_ORDER`], and it is printed at the
//! top of every plan output. A pinned order is worth more than a clever one:
//! it is checkable.

use columnar_core::{ComparisonOp, DataType, Predicate, ScalarValue};

use crate::{Dataset, EngineError};

/// The pinned optimisation order, printed at the top of every plan output.
pub const OPTIMIZATION_ORDER: &str =
    "predicate pushdown -> projection pushdown -> late materialization -> aggregate";

/// What a plan needs to know about the columns it may touch.
///
/// The plan resolves against this rather than against a decoded dataset, so
/// that a query over a Parquet file can be checked -- an unknown column, a
/// type mismatch -- before a single row is read. The eager path resolves
/// against a [`Dataset`], which implements this trait over its chunks; the
/// lazy path resolves against the file's schema.
pub trait ColumnSchema {
    /// How many columns the schema has.
    fn column_count(&self) -> usize;

    /// The name of the column at `index`.
    ///
    /// An index outside the schema yields an empty name, which no plan can
    /// name, so a plan that reaches it fails rather than matching something.
    fn column_name(&self, index: usize) -> &str;

    /// The type of the column at `index`.
    fn column_type(&self, index: usize) -> DataType;
}

/// The aggregate functions the engine supports.
#[derive(Debug, Clone, PartialEq)]
pub enum Aggregate {
    /// SUM of a column. NULL values are skipped, as in SQL.
    Sum {
        /// The column to sum.
        column: String,
    },
    /// COUNT of a non-NULL value in a column.
    Count {
        /// The column to count.
        column: String,
    },
    /// COUNT(*) over the group.
    CountStar,
    /// MIN of a column, ignoring NULLs.
    Min {
        /// The column to take the minimum of.
        column: String,
    },
    /// MAX of a column, ignoring NULLs.
    Max {
        /// The column to take the maximum of.
        column: String,
    },
}

/// The aggregation step of a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateSpec {
    /// Columns to group by. Empty means a single global group.
    pub group_by: Vec<String>,
    /// The aggregates to compute over each group.
    pub aggregates: Vec<Aggregate>,
}

/// A query plan. There is no SQL parser; the caller builds this in Rust.
///
/// The projection lists the columns to materialise. An empty projection
/// means "all columns" for a plan without an aggregation. A plan *with* an
/// aggregation ignores the projection: the aggregate result is the output, and
/// late materialisation keeps the decoded set to the filter and aggregate
/// columns.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryPlan {
    /// The predicates the plan filters on, pushed down into the scan.
    ///
    /// Every predicate must hold for a row to survive. An empty list means
    /// "no filter": every row survives.
    pub filter: Vec<Predicate>,
    /// Columns to materialise. Empty means all columns.
    pub projection: Vec<String>,
    /// Optional aggregation, applied last.
    pub aggregate: Option<AggregateSpec>,
}

impl QueryPlan {
    /// A plan that scans every column and returns every row.
    #[must_use]
    pub fn scan_all() -> Self {
        Self { filter: Vec::new(), projection: Vec::new(), aggregate: None }
    }

    /// Resolves the plan's column names to indices, once, up front.
    ///
    /// Every column the plan mentions is looked up here, so a plan that names
    /// a column the dataset does not have fails before a single row is read.
    /// The resolution also type-checks the filter and the aggregates: an
    /// `int64` column compared against a `float64` threshold is a plan error
    /// rather than an implicit cast.
    pub fn resolve(&self, dataset: &Dataset) -> Result<ResolvedPlan, EngineError> {
        self.resolve_against(dataset)
    }

    /// Resolves the plan against a schema.
    ///
    /// This is [`QueryPlan::resolve`] against something that is not a decoded
    /// dataset -- a Parquet file's schema, say -- so that a query can be
    /// checked before any data is read. The lazy path uses it, and it is the
    /// reason the lazy path can report an unknown column without having
    /// touched the file's data.
    pub fn resolve_against(&self, schema: &dyn ColumnSchema) -> Result<ResolvedPlan, EngineError> {
        let filter = self
            .filter
            .iter()
            .map(|predicate| resolve_filter(predicate, schema))
            .collect::<Result<Vec<ResolvedFilter>, EngineError>>()?;
        let aggregate = match &self.aggregate {
            Some(spec) => Some(resolve_aggregate(spec, schema)?),
            None => None,
        };
        let projection = resolve_projection(&self.projection, schema)?;

        let materialised = materialised_columns(schema, aggregate.as_ref(), &projection);

        Ok(ResolvedPlan { filter, projection, aggregate, materialised })
    }
}

/// Resolves the filter's column and type-checks its threshold.
fn resolve_filter(
    predicate: &Predicate,
    schema: &dyn ColumnSchema,
) -> Result<ResolvedFilter, EngineError> {
    let column = column_index(schema, &predicate.column)?;
    let column_type = schema.column_type(column);
    let value_type = predicate.value.data_type();
    if column_type != value_type {
        return Err(EngineError::TypeMismatch(format!(
            "column `{}` is {} but the predicate compares it against a {} threshold",
            predicate.column,
            column_type.name(),
            value_type.name()
        )));
    }
    Ok(ResolvedFilter { column, predicate: predicate.clone() })
}

/// Resolves the aggregate's columns and type-checks every aggregate.
fn resolve_aggregate(
    spec: &AggregateSpec,
    schema: &dyn ColumnSchema,
) -> Result<ResolvedAggregate, EngineError> {
    let group_by = spec
        .group_by
        .iter()
        .map(|name| column_index(schema, name))
        .collect::<Result<Vec<usize>, EngineError>>()?;

    let mut aggregates = Vec::with_capacity(spec.aggregates.len());
    for aggregate in &spec.aggregates {
        let function = match aggregate {
            Aggregate::Sum { column } => {
                let index = column_index(schema, column)?;
                require_numeric(schema, index, column, "SUM")?;
                ResolvedAggregateFn::Sum { column: index }
            }
            Aggregate::Count { column } => {
                let index = column_index(schema, column)?;
                ResolvedAggregateFn::Count { column: index }
            }
            Aggregate::CountStar => ResolvedAggregateFn::CountStar,
            Aggregate::Min { column } => {
                let index = column_index(schema, column)?;
                require_numeric(schema, index, column, "MIN")?;
                ResolvedAggregateFn::Min { column: index }
            }
            Aggregate::Max { column } => {
                let index = column_index(schema, column)?;
                require_numeric(schema, index, column, "MAX")?;
                ResolvedAggregateFn::Max { column: index }
            }
        };
        aggregates.push(function);
    }

    Ok(ResolvedAggregate { spec: spec.clone(), group_by, aggregates })
}

/// Resolves the projection's columns. An empty projection stays empty; it
/// means "all columns".
fn resolve_projection(
    projection: &[String],
    schema: &dyn ColumnSchema,
) -> Result<Vec<usize>, EngineError> {
    projection.iter().map(|name| column_index(schema, name)).collect()
}

/// The columns the plan will actually decode, in schema order.
///
/// This is the late-materialisation set: the filter column, the columns
/// the aggregation reads, and -- for a plan without an aggregation -- the
/// projection (or every column, when the projection is empty).
fn materialised_columns(
    schema: &dyn ColumnSchema,
    aggregate: Option<&ResolvedAggregate>,
    projection: &[usize],
) -> Vec<usize> {
    let mut columns: Vec<usize> = Vec::new();
    let mut push = |column: usize| {
        if !columns.contains(&column) {
            columns.push(column);
        }
    };

    // The filter's own column is NOT pushed here. The filter operator reads
    // it straight from the chunk -- and, for a dictionary-encoded column,
    // reads only the dictionary -- so materialising it would be work nobody
    // asked for. It is materialised only when the projection or the
    // aggregation reads it too.
    if let Some(aggregate) = aggregate {
        for &column in &aggregate.group_by {
            push(column);
        }
        for function in &aggregate.aggregates {
            if let Some(column) = function.column() {
                push(column);
            }
        }
    } else if projection.is_empty() {
        for column in 0..schema.column_count() {
            push(column);
        }
    } else {
        for &column in projection {
            push(column);
        }
    }

    columns.sort_unstable();
    columns
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
    /// The columns the plan will decode, in schema order.
    pub materialised: Vec<usize>,
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

        let materialised = self
            .materialised
            .iter()
            .filter_map(|&index| mapping.get(index).copied().flatten())
            .collect();

        ResolvedPlan { filter, projection, aggregate, materialised }
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
    /// SUM of a column.
    Sum {
        /// The index of the summed column.
        column: usize,
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
    /// The column the aggregate reads, if it reads one.
    #[must_use]
    pub const fn column(&self) -> Option<usize> {
        match self {
            Self::Sum { column }
            | Self::Count { column }
            | Self::Min { column }
            | Self::Max { column } => Some(*column),
            Self::CountStar => None,
        }
    }

    /// Maps this function's column index onto a subset of the columns.
    fn remap(&self, mapping: &[Option<usize>]) -> ResolvedAggregateFn {
        match self {
            Self::Sum { column } => Self::Sum { column: remap_index(mapping, *column) },
            Self::Count { column } => Self::Count { column: remap_index(mapping, *column) },
            Self::Min { column } => Self::Min { column: remap_index(mapping, *column) },
            Self::Max { column } => Self::Max { column: remap_index(mapping, *column) },
            Self::CountStar => Self::CountStar,
        }
    }
}

/// The index of the column called `name`.
fn column_index(schema: &dyn ColumnSchema, name: &str) -> Result<usize, EngineError> {
    (0..schema.column_count())
        .find(|&index| schema.column_name(index) == name)
        .ok_or_else(|| EngineError::UnknownColumn(name.to_owned()))
}

/// Rejects a column whose type the aggregate cannot represent.
///
/// `MIN` and `MAX` over a string column are rejected because the aggregate
/// value type has no string variant: returning one would mean inventing a
/// representation the output type cannot express.
fn require_numeric(
    schema: &dyn ColumnSchema,
    index: usize,
    name: &str,
    function: &str,
) -> Result<(), EngineError> {
    match schema.column_type(index) {
        DataType::Int64 | DataType::Float64 => Ok(()),
        DataType::Utf8 => Err(EngineError::TypeMismatch(format!(
            "{function} over the utf8 column `{name}` is not representable in the aggregate value type"
        ))),
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
