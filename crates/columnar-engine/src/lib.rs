//! The columnar engine: vectorized operators, a typed query plan and an
//! EXPLAIN-style plan output.
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
//! Both orders are stated in [`plan::OPTIMIZATION_ORDER`] and printed in the
//! plan output, so a reader never has to guess what the engine did.
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

#![deny(missing_docs)]
#![deny(unsafe_code)]
// The filter and the aggregate build bit-packed match masks, which is
// indexing by nature: a mask is written at `mask[row / 64]`. Every entry
// point checks its bounds against the dataset's row count first, and the
// mask-building code is covered by the operator tests. The lint stays
// enabled for the crates above this one.
#![allow(clippy::indexing_slicing)]

pub mod dataset;
pub mod dictionary;
pub mod explain;
pub mod memory;
pub mod operators;
pub mod plan;
pub mod pushdown;
pub mod queries;
pub mod source;

// The pipeline and the scan's decision are the engine's own machinery. They
// are modules of the crate, not of its public surface: nothing outside the
// crate names them, and the public surface is the crate root's re-exports.
pub(crate) mod pipeline;
pub(crate) mod scan;

use columnar_kernels::Flavor;

use crate::pipeline::{build_streamed_table, scan_stage, BuildSide, Pipeline, StreamedJoin};
use crate::plan::ColumnSchema;
use crate::scan::{decide_scan, fetched_columns, streamed_join, ScanReport};

pub use dataset::{Dataset, DatasetError};
pub use explain::{PlanOutput, StageStats};
pub use memory::{MemoryBudget, MemoryError};
pub use operators::aggregate::{AggregateResult, AggregateValue, Group};
pub use operators::filter::FilterStats;
pub use plan::{Aggregate, AggregateSpec, JoinSpec, QueryPlan, ResolvedAggregateFn, SortSpec};
pub use pushdown::{PushdownMetrics, PushdownPlan};
pub use queries::{BuiltinQuery, QueryDefinition, TableAddress, QUERIES};
pub use source::{ParquetSource, SourceColumn, MAX_READER_BATCH_ROWS};

/// Everything that can go wrong while executing a plan.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The plan's estimated working set does not fit the memory budget.
    ///
    /// The estimate is checked before any data is touched, so a plan that
    /// fails this check has done no work at all.
    #[error("the estimated working set exceeds the query memory budget")]
    Memory(#[from] MemoryError),
    /// The plan names a column the dataset does not have.
    #[error("unknown column `{0}`")]
    UnknownColumn(String),
    /// The plan uses a column with a type the operator cannot handle.
    ///
    /// The engine never casts silently: comparing an `int64` column against a
    /// `float64` threshold is a plan error, not something to paper over.
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    /// The plan is inconsistent: it reads a column the dataset does not have.
    ///
    /// The plan was resolved against one schema and run over another, and the
    /// two disagree. It is a plan error, and it is reported as one: a plan
    /// that cannot be run is refused, not crashed on.
    #[error("invalid plan: {0}")]
    InvalidPlan(String),
    /// The dataset could not be read.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The Parquet file could not be parsed.
    #[error("parquet error: {0}")]
    Parquet(String),
}

impl From<DatasetError> for EngineError {
    fn from(error: DatasetError) -> Self {
        match error {
            DatasetError::Io(io) => Self::Io(io),
            DatasetError::Parquet(message) => Self::Parquet(message),
            DatasetError::UnsupportedType(message) => Self::TypeMismatch(message),
        }
    }
}

/// Knobs for a query execution.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExecOptions {
    /// The per-query memory budget. Default 256 MiB.
    pub memory_budget: MemoryBudget,
    /// Which kernel flavor to use. `None` means "the fastest available".
    pub flavor: Option<Flavor>,
    /// How many threads to read the file with. `None` means one.
    ///
    /// This is the **read**'s thread count. The operators are single-threaded
    /// in this increment, and the headline comparison pins every system to
    /// one thread so the comparison is like-for-like, so the default is one.
    ///
    /// The read splits the file's **row groups** across the threads -- a row
    /// group is decoded independently of every other one -- and merges the
    /// parts back together afterwards. The rows come back in the file's order
    /// whatever this is set to, so the answer does not depend on the
    /// scheduler.
    pub threads: Option<usize>,
}

/// Executes `plan` over `dataset`.
///
/// The plan is resolved against the dataset first, so an unknown column or a
/// type mismatch is reported before any data is read. The estimated working
/// set is then checked against `options.memory_budget`; if it does not fit,
/// the call returns [`EngineError::Memory`] having done no work.
///
/// Returns [`MemoryError::BudgetExceeded`] if the plan's estimated working
/// set exceeds `options.memory_budget`.
///
/// This is the **eager** path: the dataset already holds every column of the
/// file. The lazy path is [`execute_on`], which reads only what the plan
/// needs.
pub fn execute(
    plan: &QueryPlan,
    dataset: &Dataset,
    options: &ExecOptions,
) -> Result<AggregateResult, EngineError> {
    // The clock starts before the plan is resolved: the plan output is the
    // engine's account of the **whole** query, and the plan's resolution is
    // part of it.
    let started = std::time::Instant::now();
    let resolved = plan.resolve(dataset)?;
    let build = BuildSide::open(resolved.join.as_ref())?;

    let grouped =
        resolved.aggregate.as_ref().is_some_and(|aggregate| !aggregate.group_by.is_empty());
    let estimate = memory::estimate_resolved(&resolved, dataset, grouped)
        .saturating_add(memory::estimate_plan_join(resolved.join.as_ref(), build.source()))
        .saturating_add(memory::estimate_plan_sort(resolved.sort.as_ref(), dataset.len()));
    if estimate > options.memory_budget.bytes {
        return Err(EngineError::Memory(MemoryError::BudgetExceeded));
    }

    // The eager path read the whole file before the clock started, so the
    // scan's own stage is empty: it cost nothing here.
    let scan = scan_stage(dataset.len(), dataset.len(), std::time::Duration::ZERO);

    let report = ScanReport {
        pushed_down: Vec::new(),
        columns_read: dataset.column_names(),
        pushdown_metrics: PushdownMetrics::default(),
    };

    // The eager path reads the whole file before the plan runs, so the join's
    // probe cannot happen inside the scan: there is no scan to feed.
    Pipeline {
        resolved: &resolved,
        dataset,
        options,
        // The flavor is resolved once, here, and the pipeline runs the same
        // kernels the caller asked for.
        flavor: options.flavor.unwrap_or_else(columnar_kernels::best),
        build: &build,
        started,
        scan,
        report,
        estimate,
        workers: 1,
        streamed: None,
    }
    .run()
}

/// Executes `plan` over a Parquet file, reading only what the plan needs.
///
/// This is the **lazy** path. It differs from [`execute`] in three ways, and
/// each of them is the point:
///
/// - **The plan resolves against the file's schema**, so an unknown column
///   or a type mismatch is reported before a single row is read.
/// - **Only the columns the plan needs are read.** The bytes of the others
///   are never fetched from the file.
/// - **The plan's predicates are pushed down into the scan**, so the rows
///   that fail them are dropped during the scan rather than after it. The
///   engine evaluates them itself and keeps the mask; the reader is given
///   no row filter, because a row filter is a list of row ranges and the
///   list is longer than the mask. See
///   [ADR-0015](../../../docs/adr/0015-the-engine-keeps-the-mask.md).
/// - **The scan copies each surviving row once.** The survivors are written
///   straight out of the reader's array into the engine's own column: the
///   batch is not filtered first, and a column the query does not
///   materialise is never copied. See
///   [ADR-0017](../../../docs/adr/0017-the-scan-filters-only-what-it-keeps.md).
///
/// The pushdown reproduces the engine's filter semantics exactly -- a NULL
/// never matches, a NaN never matches, for every operator including `!=` --
/// which is stricter than both SQL and Arrow's own comparisons. See
/// [`pushdown`] and [ADR-0010](../../../docs/adr/0010-pushdown-equivalence.md).
///
/// # Errors
///
/// Returns [`EngineError::Memory`] when the plan's estimated working set
/// exceeds `options.memory_budget`, and the usual resolution errors when
/// the plan does not fit the file's schema.
pub fn execute_on(
    plan: &QueryPlan,
    source: &ParquetSource,
    options: &ExecOptions,
) -> Result<AggregateResult, EngineError> {
    // The clock starts before the plan is resolved: the plan output is the
    // engine's account of the **whole** query, and the plan's resolution is
    // part of it.
    let started = std::time::Instant::now();
    let resolved = plan.resolve_against(source)?;
    let decision = decide_scan(&resolved, source);
    let build = BuildSide::open(resolved.join.as_ref())?;

    // The same flavor the filter would use, so the pushdown and the filter
    // run the same kernel over the same values.
    let flavor = options.flavor.unwrap_or_else(columnar_kernels::best);

    // How many threads will *actually* read. The read splits the file's row
    // groups across the threads, so a file with six row groups has six
    // workers however many threads are asked for. The plan output reports
    // this number rather than the requested one, because a plan that claims
    // fourteen threads and used six is a plan that is telling a lie.
    let workers = source.worker_count(options.threads.unwrap_or(1));

    // The budget is checked **before** the read. The estimate is made from
    // the file's schema and its row count, so a plan that does not fit is
    // refused having read nothing at all -- and it accounts for the parallel
    // read's peak, which holds two copies of the read set rather than one.
    // The join's build side and the sort's working set are in the estimate
    // too: a plan that does not fit is refused before any of it is built.
    let grouped =
        resolved.aggregate.as_ref().is_some_and(|aggregate| !aggregate.group_by.is_empty());
    let estimate = memory::estimate_lazy_read(source, &decision.read, workers, grouped)
        .saturating_add(memory::estimate_plan_join(resolved.join.as_ref(), build.source()))
        .saturating_add(memory::estimate_plan_sort(resolved.sort.as_ref(), source.rows()));
    if estimate > options.memory_budget.bytes {
        return Err(EngineError::Memory(MemoryError::BudgetExceeded));
    }

    // Does the scan feed the join?
    //
    // The scan feeds the join when the join's probe can happen **before** the
    // copy: the plan has a join, every predicate was pushed into the reader
    // -- so the filter stage has no work to do -- and the join's key is a
    // type the probe reads. See ADR-0030.
    let streamed = streamed_join(&resolved, &decision, source);

    // The join's table is built **before** the scan when the scan feeds the
    // join: the probe has to happen before the copy, and the probe needs the
    // table.
    let table = build_streamed_table(&resolved, &build, options, flavor, streamed)?;

    // The clock starts **after** the build table is built: the table is the
    // join's work, and the join's stage reports it.
    let scan_started = std::time::Instant::now();
    let (dataset, joined, filter_elapsed, examined) = match &table {
        Some(table) => {
            let filter = operators::join::JoinFilter::new(&table.table, table.key);
            let output = source.read_filtered(
                &decision.read,
                &decision.pushdown,
                &filter,
                flavor,
                workers,
            )?;
            (output.dataset, Some(output.rows), output.filter_elapsed, output.examined)
        }
        None => (
            source.read(&decision.read, &decision.pushdown, flavor, workers)?,
            None,
            std::time::Duration::ZERO,
            0,
        ),
    };
    let scan_elapsed = scan_started.elapsed();

    // Every index the plan holds is a column of the *file*; the dataset holds
    // only the read set, so the indices have to move.
    let mapping = decision.mapping(source.column_count());
    let effective = resolved.remap(&mapping)?;

    // The filter ran inside the scan, but it is the **join's** work, and the
    // join's stage reports it.
    let scan =
        scan_stage(source.rows(), dataset.len(), scan_elapsed.saturating_sub(filter_elapsed));

    let report = ScanReport {
        pushed_down: decision.pushdown_descriptions(),
        columns_read: source.column_names(&fetched_columns(&decision)),
        pushdown_metrics: decision.pushdown.metrics(),
    };

    let build_elapsed = table.as_ref().map_or(std::time::Duration::ZERO, |table| table.elapsed);
    let build_rows = table.as_ref().map_or(0, |table| table.table.rows());
    let streamed = joined
        .map(|rows| StreamedJoin::new(rows, build_elapsed + filter_elapsed, build_rows, examined));

    Pipeline {
        resolved: &effective,
        dataset: &dataset,
        options,
        // The same flavor the read used, so the operators run the same
        // kernels over the same values.
        flavor,
        build: &build,
        started,
        scan,
        report,
        estimate,
        workers,
        streamed,
    }
    .run()
}
