//! The columnar engine's command line interface, as a library.
//!
//! The binary is a thin wrapper around this: the subcommand definitions live
//! in `main.rs`, and the *query definitions* live here, because they are the
//! thing the tests need to reach.
//!
//! The queries are the project's contract with a reader: the TPC-H Q6
//! predicate set, the filter-only queries, the aggregate-only query. Keeping
//! them in a library means the end-to-end test can run **the same plans** the
//! benchmark runs, rather than a copy of them that could drift.

pub mod queries;

pub use queries::{BuiltinQuery, QueryDefinition, QUERIES};
