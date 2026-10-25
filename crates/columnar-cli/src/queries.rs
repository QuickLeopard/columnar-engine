//! The builtin queries.
//!
//! These are the queries the engine and the incumbents are compared on. Each
//! one exists to exercise a specific mechanism, and each one is written so
//! that the *same* SQL can be handed to `DuckDB` and to `Polars`.
//!
//! ## Why the last query counts rows instead of summing
//!
//! The TPC-H Q6 query computes `sum(l_extendedprice * (1 - l_discount))` --
//! a sum over a *computed* expression. This milestone's aggregate operator
//! sums a column; it does not evaluate arithmetic over columns. So the Q6
//! comparison here uses **Q6's predicate** with `count(*)` in place of the
//! computed sum.
//!
//! That is a deliberate, stated limitation rather than a quiet one: the
//! filter half of Q6 -- a range predicate on a numeric column `AND`ed with a
//! range predicate on a dictionary-encoded string column -- is exactly what
//! this engine claims to do well, and it is measured honestly. The aggregate
//! half is a separate increment.

use columnar_core::{ComparisonOp, Predicate, ScalarValue};
use columnar_engine::{Aggregate, AggregateSpec, QueryPlan};

/// The table name the `Polars` driver registers the Parquet file under.
///
/// It must match the default in `tools/polars-run.py`. `Polars`' `SQLContext`
/// cannot read a file path from SQL, so the driver registers the scan under a
/// name and the SQL refers to that name.
const POLARS_TABLE: &str = "lineitem";

/// The queries the engine knows how to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinQuery {
    /// `SELECT count(*) FROM lineitem` -- the COUNT(*) fast path, with no
    /// column materialised at all.
    Scan,
    /// One numeric predicate and a COUNT(*).
    FilterNumeric,
    /// One predicate on a dictionary-encoded string column and a COUNT(*).
    FilterString,
    /// Two numeric predicates `AND`ed, and a SUM.
    FilterRange,
    /// The TPC-H Q6 predicate set, with COUNT(*) in place of the computed sum.
    Q6,
}

impl BuiltinQuery {
    /// The query's short name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::FilterNumeric => "filter-numeric",
            Self::FilterString => "filter-string",
            Self::FilterRange => "filter-range",
            Self::Q6 => "q6",
        }
    }
}

/// A query the engine can run, with the SQL that means the same thing to an
/// incumbent.
#[derive(Debug, Clone, Copy)]
pub struct QueryDefinition {
    /// Which query this is.
    pub query: BuiltinQuery,
    /// The SQL, with `{table}` standing in for the table or file.
    pub sql: &'static str,
    /// Builds the engine's plan.
    pub plan: fn() -> QueryPlan,
}

impl QueryDefinition {
    /// The SQL as the engine runs it, with the table named rather than the
    /// file.
    #[must_use]
    pub fn sql_for_display(&self) -> String {
        self.sql.replace("{table}", "lineitem")
    }

    /// The SQL for an incumbent reading `path`.
    ///
    /// The three systems address a Parquet file differently, and the
    /// difference is not cosmetic:
    ///
    /// - `DuckDB` in **cold** mode reads a Parquet file straight from SQL
    ///   through a quoted path, so the table is the file.
    /// - `DuckDB` in **resident** mode has already loaded the file into a
    ///   temporary table named `lineitem`, so the table is that table.
    /// - `Polars`' `SQLContext` has no file-path relation at all. Its driver
    ///   registers the file under a table name and the SQL refers to that
    ///   name, so the table is `lineitem` in both modes.
    ///
    /// Every one of them reads the same bytes. Handing `Polars` a quoted path
    /// -- as this function used to do -- makes it fail with *"relation not
    /// found"*, which is a different question from the one being measured.
    #[must_use]
    pub fn sql_for(
        &self,
        comparator: columnar_bench::Comparator,
        mode: columnar_bench::Mode,
        path: &std::path::Path,
    ) -> String {
        match (comparator, mode) {
            (columnar_bench::Comparator::DuckDb, columnar_bench::Mode::Cold) => {
                self.sql.replace("{table}", &format!("'{}'", path.display()))
            }
            (columnar_bench::Comparator::DuckDb, columnar_bench::Mode::Resident)
            | (columnar_bench::Comparator::Polars, _) => self.sql.replace("{table}", POLARS_TABLE),
        }
    }
}

/// Every builtin query.
pub const QUERIES: &[QueryDefinition] = &[
    QueryDefinition {
        query: BuiltinQuery::Scan,
        sql: "SELECT count(*) FROM {table}",
        plan: scan_plan,
    },
    QueryDefinition {
        query: BuiltinQuery::FilterNumeric,
        sql: "SELECT count(*) FROM {table} WHERE l_discount >= 0.04",
        plan: filter_numeric_plan,
    },
    QueryDefinition {
        query: BuiltinQuery::FilterString,
        sql: "SELECT count(*) FROM {table} WHERE l_shipdate >= '1994-01-01'",
        plan: filter_string_plan,
    },
    QueryDefinition {
        query: BuiltinQuery::FilterRange,
        sql: "SELECT sum(l_quantity) FROM {table} WHERE l_discount >= 0.04 AND l_discount <= 0.07",
        plan: filter_range_plan,
    },
    QueryDefinition {
        query: BuiltinQuery::Q6,
        sql: "SELECT count(*) FROM {table} WHERE l_discount >= 0.04 AND l_discount <= 0.07 \
               AND l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01'",
        plan: q6_plan,
    },
];

/// `SELECT count(*)` over every row.
fn scan_plan() -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    }
}

/// A single numeric predicate.
fn filter_numeric_plan() -> QueryPlan {
    QueryPlan {
        filter: vec![Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04))],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    }
}

/// A single predicate on a dictionary-encoded string column.
fn filter_string_plan() -> QueryPlan {
    QueryPlan {
        filter: vec![Predicate::new(
            "l_shipdate",
            ComparisonOp::GtEq,
            ScalarValue::Utf8("1994-01-01".to_owned()),
        )],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    }
}

/// Two numeric predicates `AND`ed.
fn filter_range_plan() -> QueryPlan {
    QueryPlan {
        filter: vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)),
            Predicate::new("l_discount", ComparisonOp::LtEq, ScalarValue::Float64(0.07)),
        ],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { column: "l_quantity".to_owned() }],
        }),
    }
}

/// The TPC-H Q6 predicate set.
fn q6_plan() -> QueryPlan {
    QueryPlan {
        filter: vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)),
            Predicate::new("l_discount", ComparisonOp::LtEq, ScalarValue::Float64(0.07)),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::GtEq,
                ScalarValue::Utf8("1994-01-01".to_owned()),
            ),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::Lt,
                ScalarValue::Utf8("1995-01-01".to_owned()),
            ),
        ],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    }
}
