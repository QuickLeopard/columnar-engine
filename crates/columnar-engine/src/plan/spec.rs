//! The plan the caller writes.
//!
//! This is the engine's query surface: a typed struct, built in Rust, with
//! no parser in front of it. Every column is named by name here, and every
//! name is resolved to an index once, up front, by
//! [`crate::plan::resolve`].

use std::path::PathBuf;

use columnar_core::{Expr, Predicate};

use crate::{Dataset, EngineError};

use super::resolved::ResolvedPlan;

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
}
