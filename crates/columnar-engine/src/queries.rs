//! The builtin queries.
//!
//! These are the queries the engine and the incumbents are compared on. Each
//! one exists to exercise a specific mechanism, and each one is written so
//! that the *same* SQL can be handed to `DuckDB` and to `Polars`.
//!
//! ## The two Q6 queries
//!
//! There are two, and they are different queries.
//!
//! - **`q6`** is Q6's *predicate* with `count(*)` in place of the computed
//!   sum. It is the filter half of Q6 -- a range predicate on a numeric column
//!   `AND`ed with a range predicate on a dictionary-encoded string column --
//!   and it is what the repository measured for three increments.
//! - **`q6-revenue`** is Q6 itself: the same shape of predicate, over a
//!   tighter range, and `sum(l_extendedprice * (1 - l_discount))` as the
//!   aggregate -- a sum over an *arithmetic expression*.
//!
//! `q6` stays because it is the query the published tables are the median of,
//! and changing a published query's meaning would make every published figure
//! mean something else. `q6-revenue` is added because the aggregate half of
//! Q6 is now implemented, and a repository that measures "Q6's predicate with
//! `count(*)`" and calls it Q6 is measuring something else.
//!
//! The two are measured side by side, so the cost of the arithmetic is a
//! difference between two published numbers rather than an assertion.
//!
//! ## Why they live here
//!
//! The definitions used to live in the command line crate, because the command
//! line crate is where the SQL is handed to the incumbents. But a query is not
//! a command line argument: it is a **plan**, and the plan belongs to the
//! engine that executes it. The probe in `crates/columnar-engine/examples/`
//! measures the *real* Q6, and it could not reach a plan that lived in the
//! crate above it -- so it was measuring a hand-built stand-in, and calling it
//! "the whole query".
//!
//! So the engine owns the queries, and the command line crate owns only the
//! part that is about the incumbents: how the SQL addresses the table.
//!
//! ## How the SQL addresses the table
//!
//! The three systems address a Parquet file differently, and the difference is
//! not cosmetic:
//!
//! - `DuckDB` in **cold** mode reads a Parquet file straight from SQL through
//!   a quoted path, so the table is the file.
//! - `DuckDB` in **resident** mode has already loaded the file into a
//!   temporary table named `lineitem`, so the table is that table.
//! - `Polars`' `SQLContext` has no file-path relation at all. Its driver
//!   registers the file under a table name and the SQL refers to that name.
//! - The `DataFusion` peer registers the file under the same name.
//!
//! Every one of them reads the same bytes. Handing `Polars` a quoted path
//! -- as this code used to do -- makes it fail with *"relation not found"*,
//! which is a different question from the one being measured.
//!
//! [`TableAddress`] is that decision, and it is the engine's: the engine knows
//! what a table is called, and the command line crate knows which system is
//! being driven.

use std::path::Path;

use columnar_core::{ArithOp, ComparisonOp, Expr, Predicate, ScalarValue};

use crate::plan::{Aggregate, AggregateSpec, JoinSpec, QueryPlan, SortSpec};

/// The name the table is registered under.
///
/// It must match the default in `tools/polars-run.py` and the name the
/// `DataFusion` peer registers its table under.
pub const TABLE_NAME: &str = "lineitem";

/// The name the **build** table is registered under.
///
/// The join reads two files, and the second one needs a name as well: the
/// incumbents register their tables under a name, and the SQL names them.
pub const BUILD_TABLE_NAME: &str = "orders";

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
    /// TPC-H Q6 itself: the revenue query, with an arithmetic aggregate.
    Q6Revenue,
    /// The hash join: `lineitem` joined to `orders` on the order key.
    JoinOrders,
    /// The top-k sort: the ten most expensive line items.
    SortTopK,
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
            Self::Q6Revenue => "q6-revenue",
            Self::JoinOrders => "join-orders",
            Self::SortTopK => "sort-topk",
        }
    }
}

/// A query the engine can run, with the SQL that means the same thing to an
/// incumbent.
#[derive(Debug, Clone, Copy)]
pub struct QueryDefinition {
    /// Which query this is.
    pub query: BuiltinQuery,
    /// The SQL, with `{table}` standing in for the table or file and
    /// `{build}` for the second one.
    pub sql: &'static str,
    /// Builds the engine's plan.
    ///
    /// The builder takes the **build table's file**. The join reads it, and
    /// the queries that read one table ignore it -- so the signature is the
    /// same for every query, and the driver does not have to know which
    /// queries need a second file.
    pub plan: fn(&Path) -> QueryPlan,
}

impl QueryDefinition {
    /// The SQL as the engine runs it, with the tables named rather than the
    /// files.
    #[must_use]
    pub fn sql_for_display(&self) -> String {
        self.sql_for(TableAddress::Named(TABLE_NAME), TableAddress::Named(BUILD_TABLE_NAME))
    }

    /// The SQL for a system that addresses the tables in a particular way.
    ///
    /// The two addresses are the primary table's and the build table's. A
    /// query that reads one table names only the first, and the second is
    /// ignored -- but the signature is the same for every query, so the
    /// driver hands over the two files it has and moves on.
    #[must_use]
    pub fn sql_for(&self, table: TableAddress<'_>, build: TableAddress<'_>) -> String {
        let sql = self.sql.replace("{table}", &address_of(table));
        sql.replace("{build}", &address_of(build))
    }
}

/// How a system's SQL names a table.
fn address_of(address: TableAddress<'_>) -> String {
    match address {
        TableAddress::File(path) => format!("'{}'", path.display()),
        TableAddress::Named(name) => name.to_owned(),
    }
}

/// How a system's SQL names the table the query reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAddress<'a> {
    /// The table is the Parquet file itself, quoted into the SQL.
    File(&'a Path),
    /// The table has this name, and the system has already loaded the file
    /// under it.
    Named(&'a str),
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
    QueryDefinition {
        query: BuiltinQuery::Q6Revenue,
        sql: "SELECT sum(l_extendedprice * (1 - l_discount)) FROM {table} \
               WHERE l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01' \
               AND l_discount >= 0.05 AND l_discount <= 0.07 AND l_quantity < 24",
        plan: q6_revenue_plan,
    },
    QueryDefinition {
        query: BuiltinQuery::JoinOrders,
        sql: "SELECT sum(l.l_extendedprice) FROM {table} l JOIN {build} o \
               ON l.l_orderkey = o.o_orderkey \
               WHERE o.o_orderdate >= '1994-01-01' AND o.o_orderdate < '1995-01-01'",
        plan: join_orders_plan,
    },
    QueryDefinition {
        query: BuiltinQuery::SortTopK,
        sql: "SELECT sum(l_extendedprice) FROM \
               (SELECT l_extendedprice FROM {table} ORDER BY l_extendedprice DESC LIMIT 10) t",
        plan: sort_topk_plan,
    },
];

/// `SELECT count(*)` over every row.
fn scan_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// A single numeric predicate.
fn filter_numeric_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(
        vec![Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04))],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// A single predicate on a dictionary-encoded string column.
fn filter_string_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(
        vec![Predicate::new(
            "l_shipdate",
            ComparisonOp::GtEq,
            ScalarValue::Utf8("1994-01-01".to_owned()),
        )],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// Two numeric predicates `AND`ed.
fn filter_range_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(
        vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)),
            Predicate::new("l_discount", ComparisonOp::LtEq, ScalarValue::Float64(0.07)),
        ],
        Vec::new(),
        Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("l_quantity") }],
        }),
    )
}

/// The TPC-H Q6 predicate set.
fn q6_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(
        vec![
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
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// TPC-H Q6 itself: the revenue query.
///
/// `sum(l_extendedprice * (1 - l_discount))` over the rows whose discount is
/// in `[0.05, 0.07]`, whose ship date is in 1994 and whose quantity is under
/// 24.
///
/// The aggregate is an **expression**, not a column: the plan carries
/// `l_extendedprice * (1 - l_discount)`, the engine evaluates it over the
/// rows that survived the filter, and the sum is over the result.
fn q6_revenue_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(
        vec![
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
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05)),
            Predicate::new("l_discount", ComparisonOp::LtEq, ScalarValue::Float64(0.07)),
            Predicate::new("l_quantity", ComparisonOp::Lt, ScalarValue::Float64(24.0)),
        ],
        Vec::new(),
        Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(
                    ArithOp::Mul,
                    Expr::column("l_extendedprice"),
                    Expr::binary(ArithOp::Sub, Expr::constant(1.0), Expr::column("l_discount")),
                ),
            }],
        }),
    )
}

/// The hash join: `lineitem` joined to `orders` on the order key.
///
/// The **build side** is `orders`, and its own filter -- the 1994 order
/// dates -- is applied before the hash table is built, so a row outside 1994
/// is never in the table and never matches anything. The **probe side** is
/// `lineitem`, and the aggregate is over the probe side's column, which is
/// what the SQL says.
///
/// The alias sits after the relation, because in cold mode the relation is a
/// quoted file path and `'<path>' l` is not a relation with an alias.
fn join_orders_plan(build: &Path) -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("l_extendedprice") }],
        }),
        join: Some(JoinSpec {
            build_path: build.to_path_buf(),
            probe_key: "l_orderkey".to_owned(),
            build_key: "o_orderkey".to_owned(),
            build_filter: vec![
                Predicate::new(
                    "o_orderdate",
                    ComparisonOp::GtEq,
                    ScalarValue::Utf8("1994-01-01".to_owned()),
                ),
                Predicate::new(
                    "o_orderdate",
                    ComparisonOp::Lt,
                    ScalarValue::Utf8("1995-01-01".to_owned()),
                ),
            ],
        }),
        sort: None,
    }
}

/// The top-k sort: the ten most expensive line items.
///
/// The sort runs over every row of the file and keeps ten of them, so the
/// plan's sort holds a **bounded heap** of ten rows rather than sorting six
/// million. The aggregate is over the sorted rows -- the ten the sort kept.
fn sort_topk_plan(_build: &Path) -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: vec!["l_extendedprice".to_owned()],
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("l_extendedprice") }],
        }),
        join: None,
        sort: Some(SortSpec {
            column: "l_extendedprice".to_owned(),
            descending: true,
            limit: Some(10),
        }),
    }
}
