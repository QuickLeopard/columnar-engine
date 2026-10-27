//! The builtin queries, as the command line drives them.
//!
//! The queries themselves live in the engine crate, because a query is a plan
//! and a plan belongs to the engine that executes it. What is left here is the
//! part that is about the **incumbents**: how the SQL addresses the table each
//! of them reads.
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

use std::path::Path;

use columnar_bench::{Comparator, Mode};

pub use columnar_engine::queries::{
    BuiltinQuery, QueryDefinition, TableAddress, BUILD_TABLE_NAME, QUERIES, TABLE_NAME,
};

/// The SQL for an incumbent reading `path`.
///
/// The address is the decision, and it is made once, here, so that every
/// incumbent is driven the same way. The join's second table is addressed the
/// same way as the first: a path for cold `DuckDB`, a name for everything
/// else.
#[must_use]
pub fn sql_for(
    definition: &QueryDefinition,
    comparator: Comparator,
    mode: Mode,
    path: &Path,
    build: &Path,
) -> String {
    let address = match (comparator, mode) {
        // Cold `DuckDB` reads the Parquet file directly from the path.
        (Comparator::DuckDb, Mode::Cold) => TableAddress::File(path),
        // Everything else has already registered the file under a name.
        _ => TableAddress::Named(TABLE_NAME),
    };
    let build_address = match (comparator, mode) {
        (Comparator::DuckDb, Mode::Cold) => TableAddress::File(build),
        _ => TableAddress::Named(BUILD_TABLE_NAME),
    };
    definition.sql_for(address, build_address)
}
