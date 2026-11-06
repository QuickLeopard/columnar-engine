//! The columnar engine: a typed query plan over Parquet, as a library.
//!
//! The engine reads a Parquet file into [`Dataset`]s of column chunks and
//! executes a [`QueryPlan`] over them. The pipeline is fixed and pinned:
//!
//! ```text
//! scan -> filter -> project -> [join] -> [sort] -> aggregate
//! ```
//!
//! and the optimisation order is pinned as well:
//!
//! ```text
//! predicate pushdown -> projection pushdown -> late materialization -> join -> sort -> aggregate
//! ```
//!
//! Both orders are stated in `plan::OPTIMIZATION_ORDER` and printed in the
//! plan output, so a reader never has to guess what the engine did.
//!
//! ## The two entry points
//!
//! - [`execute_on`] is the **lazy** path: it reads only the columns the plan
//!   needs, pushes the plan's predicates into the scan, and resolves the plan
//!   against the file's schema before a row is read.
//! - [`execute`] is the **eager** path: it runs the same plan over a dataset
//!   that is already decoded.
//!
//! Both return an [`AggregateResult`], which holds the groups, the per-stage
//! statistics, and the rendered [`PlanOutput`].
//!
//! ## A worked example
//!
//! The example writes its own Parquet file, so it runs anywhere:
//!
//! ```
//! use columnar_engine::prelude::*;
//!
//! // A one-column Parquet file with four rows.
//! let path = std::env::temp_dir().join("columnar-engine-example.parquet");
//! {
//!     use arrow::array::Int64Array;
//!     use arrow::datatypes::{DataType, Field, Schema};
//!     use arrow::record_batch::RecordBatch;
//!     use parquet::arrow::ArrowWriter;
//!     use std::sync::Arc;
//!
//!     let schema = Schema::new(vec![Field::new("l_quantity", DataType::Int64, false)]);
//!     let batch = RecordBatch::try_new(
//!         schema.clone().into(),
//!         vec![Arc::new(Int64Array::from(vec![1_i64, 2, 3, 4]))],
//!     )?;
//!     let file = std::fs::File::create(&path)?;
//!     let mut writer = ArrowWriter::try_new(file, Arc::new(schema), None)?;
//!     writer.write(&batch)?;
//!     writer.close()?;
//! }
//!
//! // Open the file, and plan over it.
//! let source = open_source(&path)?;
//! let plan = QueryPlan::new(
//!     vec![col("l_quantity").gt(2)],
//!     Vec::new(),
//!     Some(AggregateSpec {
//!         group_by: Vec::new(),
//!         aggregates: vec![Aggregate::CountStar],
//!     }),
//! );
//!
//! // Run it.
//! let result = execute_on(&plan, &source, &ExecOptions::default())?;
//!
//! // Two of the four rows are greater than two.
//! assert_eq!(result.groups[0].values[0], AggregateValue::Int(2));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## The join and the sort
//!
//! The two optional stages are a **hash join** and a **sort**.
//!
//! - The join is an inner equi-join on one key column. The **build side** is
//!   the right table: it is read through the engine's own source, its own
//!   predicates are applied before the table is hashed, and the surviving rows
//!   are hashed on the key. The **probe side** is the left table, and it
//!   stays lazy -- the plan's predicates are pushed into its scan, and it is
//!   never materialised as a table of its own. A NULL key never matches, on
//!   either side, and the output keeps the probe side's row order.
//! - The sort is `ORDER BY <column> [DESC] [LIMIT n]`. With a limit it keeps
//!   a bounded heap of `n` rows rather than sorting everything; without one
//!   it sorts everything. Ties are broken by the row index, so the order is
//!   deterministic.
//!
//! A plan with a sort and an aggregation aggregates over the **sorted** rows,
//! not over all of them.
//!
//! ## There is no SQL parser
//!
//! The query surface is the typed [`QueryPlan`] struct. A parser would be a
//! month of work that produces no evidence about the part of this project
//! that is actually under test -- the vectorized operators and the kernels
//! they call. The plan is built in Rust, and the plan output prints it, so a
//! reader sees exactly what ran.
//!
//! ## What is out of scope for this milestone
//!
//! The following are **explicitly out of scope**, not deferred silently:
//!
//! - **Morsel-driven parallelism.** The **read** is parallel: the file's row
//!   groups are split across the threads, decoded independently, and merged
//!   back in the file's order. The **operators** are not: the filter, the
//!   project, the join, the sort and the aggregate run on one thread. The
//!   operators take a `&Dataset` and return owned values, so the data path is
//!   already shaped for a future morsel loop, but no morsel driver, no task
//!   pool and no per-thread scheduling exist yet. See [spec
//!   005](../../../specs/005-parallel-decode/spec.md).
//! - **A SQL parser.** See above.
//! - **Spilling.** The memory budget is a pre-flight check, not a mechanism:
//!   a plan whose estimated working set exceeds the budget is rejected before
//!   any work is done, rather than being spilled to disk.
//! - **Window functions.** The operator set is filter, project, hash
//!   aggregate, hash join and sort.
//!
//! ## Determinism
//!
//! Two runs of the same plan over the same data produce the same output, in
//! the same order. The hash aggregate sorts its groups by key before
//! returning them, so the output does not depend on the hash seed.
//!
//! ## The surface
//!
//! Every name an embedder needs is in [`prelude`], and the crate root
//! re-exports the same list. The list is closed by one rule: **every type
//! that appears in the public signature of a surface item is on it**. The
//! research modules -- `operators`, `pushdown`, `dictionary`, `queries`,
//! `explain`, `memory` and `plan` -- are reachable and **undocumented**: they
//! are this repository's measurement harness, and a harness that cannot reach
//! the engine's internals measures nothing.

#![deny(missing_docs)]
#![deny(unsafe_code)]
// The filter and the aggregate build bit-packed match masks, which is
// indexing by nature: a mask is written at `mask[row / 64]`. Every entry
// point checks its bounds against the dataset's row count first, and the
// mask-building code is covered by the operator tests. The lint stays
// enabled for the crates above this one.
#![allow(clippy::indexing_slicing)]

pub mod facade;
pub mod prelude;

// The research modules. Reachable, because this repository's measurement
// probes, its CLI and its own tests live in the same crate and name them;
// not part of the surface, because they are not documented and not promised.
// The surface is `facade` and `prelude`, and the crate root's re-exports.
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod dictionary;
#[doc(hidden)]
pub mod explain;
#[doc(hidden)]
pub mod file_set;
#[doc(hidden)]
pub mod memory;
#[doc(hidden)]
pub mod operators;
#[doc(hidden)]
pub mod plan;
#[doc(hidden)]
pub mod pushdown;
#[doc(hidden)]
pub mod queries;
#[doc(hidden)]
pub mod source;

// The pipeline and the scan's decision are the engine's own machinery. They
// are modules of the crate, not of its public surface: nothing outside the
// crate names them, and the public surface is the crate root's re-exports.
pub(crate) mod pipeline;
pub(crate) mod scan;

// The surface, at the crate root. The same list the prelude carries, written
// out rather than globbed, because the root is the public surface and a
// public surface should be readable without following a glob. The list is
// the facade's: one source of truth, two paths to it.
pub use facade::{col, lit};
pub use facade::{
    execute, execute_on, execute_rows, execute_rows_on, open_source, open_table, Aggregate,
    AggregateResult, AggregateSpec, AggregateValue, ArithOp, Attribution, Catalog, ColumnChunk,
    ColumnExpr, ComparisonOp, DataType, Dataset, DatasetError, Encoding, EngineError, ExecOptions,
    Expr, FileSetSource, Flavor, Group, JoinReport, JoinSpec, MemoryBudget, MemoryError,
    NullBitmap, Operand, ParquetSource, PlanBuilder, PlanOutput, Predicate, PushdownMetrics,
    QueryPlan, RowGroupRange, ScalarValue, SortReport, SortSpec, SourceColumn, StageStats,
    TableAddress, MAX_READER_BATCH_ROWS,
};
