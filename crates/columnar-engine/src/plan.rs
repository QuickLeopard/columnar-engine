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
//! 4. **Join** -- the hash join runs over the rows that survived the filter,
//!    and the probe side stays lazy: it is never materialised as a table of
//!    its own.
//! 5. **Sort** -- the sort runs over the rows the join produced.
//! 6. **Aggregate** -- the aggregation runs last, over the surviving rows.
//!
//! The order is a constant, [`OPTIMIZATION_ORDER`], and it is printed at the
//! top of every plan output. A pinned order is worth more than a clever one:
//! it is checkable.

use std::path::PathBuf;

use columnar_core::{ArithOp, ComparisonOp, DataType, Expr, Predicate, ScalarValue};

use crate::{Dataset, EngineError};

/// The pinned optimisation order, printed at the top of every plan output.
pub const OPTIMIZATION_ORDER: &str =
    "predicate pushdown -> projection pushdown -> late materialization -> join -> sort -> aggregate";

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
    /// SUM of an expression. NULL values are skipped, as in SQL.
    ///
    /// The expression may be a plain column -- `Sum { expr: Expr::column(..) }`
    /// -- or an arithmetic expression over columns, which is what TPC-H's Q6
    /// needs: `sum(l_extendedprice * (1 - l_discount))`.
    ///
    /// The expression is evaluated over the rows that survived the filter, into
    /// a column, and that column is what is summed. The aggregate operator
    /// does not know the difference.
    Sum {
        /// The expression to sum.
        expr: Expr,
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

/// The join's shape.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinSpec {
    /// The Parquet file of the **build side** -- the table on the right.
    pub build_path: PathBuf,
    /// The probe side's key column: the left table's.
    pub probe_key: String,
    /// The build side's key column: the right table's.
    pub build_key: String,
    /// Predicates evaluated on the build side **before** the hash table is built.
    pub build_filter: Vec<Predicate>,
}

/// The sort's shape.
#[derive(Debug, Clone, PartialEq)]
pub struct SortSpec {
    /// The column to sort by.
    pub column: String,
    /// Whether the order is descending.
    pub descending: bool,
    /// How many rows to keep. `None` sorts everything.
    pub limit: Option<usize>,
}

/// A query plan. There is no SQL parser; the caller builds this in Rust.
///
/// The projection lists the columns to materialise. An empty projection
/// means "all columns" for a plan without an aggregation. A plan *with* an
/// aggregation ignores the projection: the aggregate result is the output, and
/// late materialisation keeps the decoded set to the filter and aggregate
/// columns.
///
/// The pipeline the plan describes is
/// `scan -> filter -> project -> [join] -> [sort] -> aggregate`, and the two
/// optional stages are the ones the plan gained last: a hash join over two
/// files, and a sort with an optional limit.
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
    /// Optional hash join, applied after the projection.
    pub join: Option<JoinSpec>,
    /// Optional sort, applied after the join.
    pub sort: Option<SortSpec>,
}

impl QueryPlan {
    /// A plan with a filter, a projection and an aggregation -- and no join
    /// and no sort.
    ///
    /// The join and the sort are the two stages the pipeline gained last, and
    /// a plan that has neither is the common case. Naming them at every call
    /// site would make the plans twice as long as they need to be, so the
    /// short form lives here and the long form is a struct literal.
    #[must_use]
    pub fn new(
        filter: Vec<Predicate>,
        projection: Vec<String>,
        aggregate: Option<AggregateSpec>,
    ) -> Self {
        Self { filter, projection, aggregate, join: None, sort: None }
    }

    /// A plan that scans every column and returns every row.
    #[must_use]
    pub fn scan_all() -> Self {
        Self::new(Vec::new(), Vec::new(), None)
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

        let join = match &self.join {
            Some(spec) => Some(ResolvedJoin {
                spec: spec.clone(),
                probe_key: column_index(schema, &spec.probe_key)?,
            }),
            None => None,
        };
        let sort = match &self.sort {
            Some(spec) => Some(ResolvedSort {
                spec: spec.clone(),
                column: column_index(schema, &spec.column)?,
            }),
            None => None,
        };

        let materialised = materialised_columns(
            schema,
            aggregate.as_ref(),
            &projection,
            join.as_ref(),
            sort.as_ref(),
        );

        Ok(ResolvedPlan { filter, projection, aggregate, join, sort, materialised })
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
            Aggregate::Sum { expr } => {
                let resolved = resolve_expr(expr, schema)?;
                ResolvedAggregateFn::Sum { expr: resolved }
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

/// Resolves an expression's column names to indices, and type-checks it.
///
/// The expression is checked **before** anything is read, so an expression
/// naming a column the file does not have is a plan error rather than a
/// surprise half way through a scan.
///
/// The type rules are the narrow ones the predicate follows: an `int64` in a
/// `float64` expression is widened, a `float64` in an `int64` expression is
/// not, and a `utf8` column is not a number at all.
fn resolve_expr(expr: &Expr, schema: &dyn ColumnSchema) -> Result<ResolvedExpr, EngineError> {
    match expr {
        Expr::Constant(value) => match value {
            ScalarValue::Int64(_) | ScalarValue::Float64(_) => {
                Ok(ResolvedExpr::Constant(value.clone()))
            }
            ScalarValue::Utf8(text) => Err(EngineError::TypeMismatch(format!(
                "the expression's constant `{text}` is a string, and an arithmetic expression computes a number"
            ))),
        },
        Expr::Column(name) => {
            let index = column_index(schema, name)?;
            match schema.column_type(index) {
                DataType::Int64 | DataType::Float64 => {
                    Ok(ResolvedExpr::Column { index, data_type: schema.column_type(index) })
                }
                DataType::Utf8 => Err(EngineError::TypeMismatch(format!(
                    "the expression reads the utf8 column `{name}`, and an arithmetic expression computes a number"
                ))),
            }
        }
        Expr::Binary { op, left, right } => {
            let left = resolve_expr(left, schema)?;
            let right = resolve_expr(right, schema)?;
            let left_type = left.data_type();
            let right_type = right.data_type();
            // `5 / 2` is `2` in PostgreSQL and `2.5` in DuckDB. The engine
            // refuses integer division rather than pick a side.
            if *op == ArithOp::Div && left_type != DataType::Float64 && right_type != DataType::Float64 {
                return Err(EngineError::TypeMismatch(
                    "division requires a float64 operand: `5 / 2` is 2 in PostgreSQL and 2.5 in DuckDB, and the engine refuses to pick a side"
                        .to_owned(),
                ));
            }
            Ok(ResolvedExpr::Binary {
                op: *op,
                left: Box::new(left),
                right: Box::new(right),
            })
        }
        Expr::Negate(inner) => {
            Ok(ResolvedExpr::Negate(Box::new(resolve_expr(inner, schema)?)))
        }
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
///
/// The join's key and the sort's column are in the set too: the stages
/// behind the projection read them, and a column nobody reads is never
/// decoded.
fn materialised_columns(
    schema: &dyn ColumnSchema,
    aggregate: Option<&ResolvedAggregate>,
    projection: &[usize],
    join: Option<&ResolvedJoin>,
    sort: Option<&ResolvedSort>,
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
            for column in function.columns() {
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

    if let Some(join) = join {
        push(join.probe_key);
    }
    if let Some(sort) = sort {
        push(sort.column);
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
