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
        let filter = self
            .filter
            .iter()
            .map(|predicate| resolve_filter(predicate, dataset))
            .collect::<Result<Vec<ResolvedFilter>, EngineError>>()?;
        let aggregate = match &self.aggregate {
            Some(spec) => Some(resolve_aggregate(spec, dataset)?),
            None => None,
        };
        let projection = resolve_projection(&self.projection, dataset)?;

        let materialised = materialised_columns(dataset, aggregate.as_ref(), &projection);

        Ok(ResolvedPlan { filter, projection, aggregate, materialised })
    }
}

/// Resolves the filter's column and type-checks its threshold.
fn resolve_filter(predicate: &Predicate, dataset: &Dataset) -> Result<ResolvedFilter, EngineError> {
    let column = column_index(dataset, &predicate.column)?;
    let chunk = chunk(dataset, column);
    let column_type = chunk.data_type();
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
    dataset: &Dataset,
) -> Result<ResolvedAggregate, EngineError> {
    let group_by = spec
        .group_by
        .iter()
        .map(|name| column_index(dataset, name))
        .collect::<Result<Vec<usize>, EngineError>>()?;

    let mut aggregates = Vec::with_capacity(spec.aggregates.len());
    for aggregate in &spec.aggregates {
        let function = match aggregate {
            Aggregate::Sum { column } => {
                let index = column_index(dataset, column)?;
                require_numeric(dataset, index, column, "SUM")?;
                ResolvedAggregateFn::Sum { column: index }
            }
            Aggregate::Count { column } => {
                let index = column_index(dataset, column)?;
                ResolvedAggregateFn::Count { column: index }
            }
            Aggregate::CountStar => ResolvedAggregateFn::CountStar,
            Aggregate::Min { column } => {
                let index = column_index(dataset, column)?;
                require_numeric(dataset, index, column, "MIN")?;
                ResolvedAggregateFn::Min { column: index }
            }
            Aggregate::Max { column } => {
                let index = column_index(dataset, column)?;
                require_numeric(dataset, index, column, "MAX")?;
                ResolvedAggregateFn::Max { column: index }
            }
        };
        aggregates.push(function);
    }

    Ok(ResolvedAggregate { spec: spec.clone(), group_by, aggregates })
}

/// Resolves the projection's columns. An empty projection stays empty; it
/// means "all columns".
fn resolve_projection(projection: &[String], dataset: &Dataset) -> Result<Vec<usize>, EngineError> {
    projection.iter().map(|name| column_index(dataset, name)).collect()
}

/// The columns the plan will actually decode, in schema order.
///
/// This is the late-materialisation set: the filter column, the columns
/// the aggregation reads, and -- for a plan without an aggregation -- the
/// projection (or every column, when the projection is empty).
fn materialised_columns(
    dataset: &Dataset,
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
        for column in 0..dataset.chunks.len() {
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
}

/// The index of the column called `name`.
fn column_index(dataset: &Dataset, name: &str) -> Result<usize, EngineError> {
    dataset
        .chunks
        .iter()
        .position(|chunk| chunk.name == name)
        .ok_or_else(|| EngineError::UnknownColumn(name.to_owned()))
}

/// The chunk at `index`.
fn chunk(dataset: &Dataset, index: usize) -> &columnar_core::ColumnChunk {
    dataset.chunks.get(index).expect("a resolved column index is in range")
}

/// Rejects a column whose type the aggregate cannot represent.
///
/// `MIN` and `MAX` over a string column are rejected because the aggregate
/// value type has no string variant: returning one would mean inventing a
/// representation the output type cannot express.
fn require_numeric(
    dataset: &Dataset,
    index: usize,
    name: &str,
    function: &str,
) -> Result<(), EngineError> {
    let data_type: DataType = chunk(dataset, index).data_type();
    match data_type {
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
