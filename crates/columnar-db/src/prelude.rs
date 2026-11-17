//! The prelude: the surface, in one import.
//!
//! ```
//! use columnar_db::prelude::*;
//! ```
//!
//! **And the engine's prelude is not needed beside this one**: the query
//! vocabulary is here, so an application imports one thing. **The two
//! preludes do share a name** — `Catalog` — and the engine's is the
//! embedder's name-to-path map while this crate's is the persistent one, so
//! an application that wants the engine's says `columnar_engine::Catalog`
//! outright.

pub use crate::catalog::Catalog;
pub use crate::database::Database;
pub use crate::row::{Row, Rows};
pub use crate::schema::{Column, Schema};
pub use crate::snapshot::{Snapshot, SnapshotTable};
pub use crate::table::{Table, TableMut};

/// **The query language is the engine's**, so the engine's query vocabulary
/// is here too: the database does not have a second one, and an application
/// should not need two imports to write a query.
pub use columnar_engine::{
    col, lit, Aggregate, AggregateSpec, ColumnExpr, ComparisonOp, DataType, ExecOptions, Expr,
    Operand, PlanBuilder, Predicate, QueryPlan, ScalarValue, TableAddress,
};
