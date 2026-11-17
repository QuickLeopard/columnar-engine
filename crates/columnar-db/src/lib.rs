//! An embedded database for the columnar engine.
//!
//! The engine reads Parquet and the store writes it. This crate is the layer
//! between them: it says which files make up which table, it gives a row an
//! identity, and it hands a read a snapshot.
//!
//! # The shape
//!
//! - A **database** is a **directory**. `catalog.json` names the tables, and
//!   `tables/` holds them — each one a **store** table, so each one a set of
//!   Parquet files with a manifest that names them.
//! - The **catalog** is replaced by a **rename**, the same primitive the
//!   store's commit uses, so a reader sees the old catalog or the new one and
//!   never a half-written one.
//! - A **table** is a **set of files**, and the engine reads a set of files as
//!   one table — `FileSetSource` is the engine's own name for it.
//!
//! # The vocabulary
//!
//! **The engine's, and only the engine's.** A column's type is one of
//! `Int64`, `Float64` or `Utf8`, and the values are the engine's
//! `ScalarValue`. **The database adds no type of its own**, because a second
//! vocabulary is a conversion at every boundary and a place for the two to
//! disagree.
//!
//! # The query language
//!
//! **The engine's typed plan.** A read is a `QueryPlan` — built by the
//! engine's own `PlanBuilder`, over the engine's own `ColumnExpr` — run
//! against the table's files. **There is no SQL parser**, and the decision is
//! recorded: SQL promises subqueries, CTEs, window functions and arbitrary
//! join graphs, and this engine refuses all four, so a SQL front end would
//! have to carry the engine's limits as a rule set just to say "not
//! supported" honestly. **A front end is a separate increment over the same
//! plan**, and nothing here has to be redone when one is wanted.
//!
//! ```
//! use columnar_db::prelude::*;
//! use columnar_engine::Catalog as EngineCatalog;
//!
//! // The plan is the engine's plan: the engine's own builder, over the
//! // engine's own catalog, and the engine's own expressions.
//! let catalog = EngineCatalog::new();
//! let plan = PlanBuilder::new(&catalog)
//!     .filter(col("id").gt(1))
//!     .projection(["id"])
//!     .build()?;
//!
//! assert!(!plan.filter.is_empty());
//! # Ok::<(), columnar_db::Error>(())
//! ```
//!
//! # What it is not
//!
//! **No SQL. No rollback. No second writer.** One writer and many readers: a
//! read sees one version of a table, and the writer is alone.
//!
//! # The increments
//!
//! **What is here now** is the database, the persistent catalog, the schema,
//! the typed rows, the write path, the snapshot read, the hidden row id and
//! the compaction: the rows are checked against the schema, buffered, and
//! written in the store's grid; a commit is the store's five steps; a read
//! sees one version of a table; a row that changes leaves a tombstone behind
//! it rather than being rewritten; and the compaction rewrites the live rows
//! into one file. **What comes next** is the probe: the catalog's open, the
//! write on the grid, the snapshot over one file and over many, the scan of a
//! point lookup, the compaction, and the space amplification. The whole of it
//! is spec 055, and the index's shape is decided by the scan's figure and not
//! by an opinion.

#![deny(missing_docs)]
#![deny(unsafe_code)]

mod chunks;
mod readers;
mod versions;

pub mod catalog;
pub mod database;
pub mod error;
pub mod row;
pub mod schema;
pub mod snapshot;
pub mod table;

pub mod prelude;

pub use catalog::{Catalog, TableEntry, FORMAT};
pub use database::{Compaction, Database, CATALOG_FILE, CATALOG_TMP, TABLES_DIR};
pub use error::{Error, Result};
pub use row::{Row, Rows};
pub use schema::{Column, Schema};
pub use snapshot::{Snapshot, SnapshotTable};
pub use table::{Table, TableMut};
