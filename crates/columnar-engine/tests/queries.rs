//! The builtin queries' SQL.
//!
//! The SQL is the contract with the incumbents: the same string is handed to
//! `DuckDB` and to `Polars`, and it has to mean the same thing as the
//! engine's plan. These tests pin it, so a change to a query's meaning
//! cannot slip through as a change to a string.

use std::path::Path;

use columnar_engine::queries::{BuiltinQuery, QueryDefinition, TableAddress, QUERIES};

/// The query with the given name.
fn query(name: BuiltinQuery) -> &'static QueryDefinition {
    QUERIES.iter().find(|definition| definition.query == name).expect("it is a builtin query")
}

/// The join's SQL is the one the spec pins.
#[test]
fn the_join_query_says_what_it_means() {
    let definition = query(BuiltinQuery::JoinOrders);
    assert_eq!(
        definition.sql,
        "SELECT sum(l.l_extendedprice) FROM {table} l JOIN {build} o \
         ON l.l_orderkey = o.o_orderkey \
         WHERE o.o_orderdate >= '1994-01-01' AND o.o_orderdate < '1995-01-01'"
    );
}

/// The sort's SQL is the one the spec pins.
#[test]
fn the_sort_query_says_what_it_means() {
    let definition = query(BuiltinQuery::SortTopK);
    assert_eq!(
        definition.sql,
        "SELECT sum(l_extendedprice) FROM \
         (SELECT l_extendedprice FROM {table} ORDER BY l_extendedprice DESC LIMIT 10) t"
    );
}

/// The alias sits **after** the relation.
///
/// In cold mode the relation is a quoted file path, and `'<path>' l` is not a
/// relation with an alias -- it is a syntax error.
#[test]
fn the_alias_sits_after_the_relation() {
    let sql = query(BuiltinQuery::JoinOrders).sql_for_display();
    assert!(sql.contains("FROM lineitem l JOIN orders o"), "the SQL is {sql}");
}

/// The tables are addressed the way the system being driven addresses them.
#[test]
fn the_tables_are_addressed_the_way_the_driver_addresses_them() {
    let definition = query(BuiltinQuery::JoinOrders);

    let named = definition.sql_for(TableAddress::Named("lineitem"), TableAddress::Named("orders"));
    assert_eq!(
        named,
        "SELECT sum(l.l_extendedprice) FROM lineitem l JOIN orders o \
         ON l.l_orderkey = o.o_orderkey \
         WHERE o.o_orderdate >= '1994-01-01' AND o.o_orderdate < '1995-01-01'"
    );

    let files = definition.sql_for(
        TableAddress::File(Path::new("/data/lineitem.parquet")),
        TableAddress::File(Path::new("/data/orders.parquet")),
    );
    assert!(
        files.contains("FROM '/data/lineitem.parquet' l JOIN '/data/orders.parquet' o"),
        "the SQL is {files}"
    );
}

/// A query that reads one table ignores the build table's address.
#[test]
fn a_query_that_reads_one_table_ignores_the_build_address() {
    let definition = query(BuiltinQuery::Q6Revenue);
    let sql = definition.sql_for(
        TableAddress::Named("lineitem"),
        TableAddress::File(Path::new("/data/orders.parquet")),
    );
    assert!(!sql.contains("orders.parquet"), "the SQL is {sql}");
    assert!(sql.contains("FROM lineitem"), "the SQL is {sql}");
}
