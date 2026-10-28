//! The bridge: a plan the caller wrote, resolved into a plan the executor
//! can read.
//!
//! This is the only module that knows both faces. It turns column names
//! into indices, type-checks the predicates and the aggregates, and decides
//! which columns the scan will decode.

use columnar_core::{ArithOp, DataType, Expr, Predicate, ScalarValue};

use crate::EngineError;

use super::resolved::{
    ResolvedAggregate, ResolvedAggregateFn, ResolvedExpr, ResolvedFilter, ResolvedJoin,
    ResolvedPlan, ResolvedSort,
};
use super::spec::{Aggregate, AggregateSpec, QueryPlan};
use super::ColumnSchema;

impl QueryPlan {
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
pub(crate) fn materialised_columns(
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
