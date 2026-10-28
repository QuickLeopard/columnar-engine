//! The typed query plan and its resolution.
//!
//! There is no SQL parser. The query surface is the [`QueryPlan`] struct,
//! built in Rust by the caller, and the plan output prints it back so a
//! reader can see exactly what ran.
//!
//! ## The pinned optimisation order
//!
//! The engine applies its optimisations in exactly this order, and no other:
//!
//! 1. **Predicate pushdown** -- the filter is evaluated inside the scan, so
//!    the rows that fail it never reach the operators behind it.
//! 2. **Projection pushdown** -- only the columns the plan reads are
//!    considered at all.
//! 3. **Late materialisation** -- of those columns, only the ones the plan
//!    actually reads are decoded. A column that is neither filtered nor
//!    aggregated is never touched, and the plan output says so by name.
//! 4. **Join** -- the hash join runs over the rows that survived the filter,
//!    and the probe side stays lazy: it is never materialised as a table of
//!    its own.
//! 5. **Sort** -- the sort runs over the rows the join produced.
//! 6. **Aggregate** -- the aggregation runs last, over the surviving rows.
//!
//! The order is a constant, [`OPTIMIZATION_ORDER`], and it is printed at the
//! top of every plan output. A pinned order is worth more than a clever one:
//! it is checkable.
//!
//! ## The plan's two faces
//!
//! The plan has two forms, and they are two modules:
//!
//! - [`spec`] holds what the **caller** writes: the plan's shape, named in
//!   the terms the caller has.
//! - [`resolved`] holds what the **executor** reads: the same plan, with
//!   every column name resolved to an index and every type checked.
//!
//! [`resolve`] is the bridge between them, and it is the only module that
//! knows both. The three depend on each other in one direction only:
//! `resolve` knows the two faces, and neither face knows `resolve`.

use columnar_core::DataType;

mod resolve;
mod resolved;
mod spec;

pub use resolved::{
    ResolvedAggregate, ResolvedAggregateFn, ResolvedExpr, ResolvedFilter, ResolvedJoin,
    ResolvedPlan, ResolvedSort,
};
pub use spec::{Aggregate, AggregateSpec, JoinSpec, QueryPlan, SortSpec};

/// The pinned optimisation order, printed at the top of every plan output.
pub const OPTIMIZATION_ORDER: &str =
    "predicate pushdown -> projection pushdown -> late materialization -> join -> sort -> aggregate";

/// What a plan needs to know about the columns it may touch.
///
/// The plan resolves against this rather than against a decoded dataset, so
/// that a query over a Parquet file can be checked -- an unknown column, a
/// type mismatch -- before a single row is read. The eager path resolves
/// against a [`Dataset`], which implements this trait over its chunks; the
/// lazy path resolves against the file's schema.
pub trait ColumnSchema {
    /// How many columns the schema has.
    fn column_count(&self) -> usize;

    /// The name of the column at `index`.
    ///
    /// An index outside the schema yields an empty name, which no plan can
    /// name, so a plan that reaches it fails rather than matching something.
    fn column_name(&self, index: usize) -> &str;

    /// The type of the column at `index`.
    fn column_type(&self, index: usize) -> DataType;
}
