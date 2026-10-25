//! The columnar engine: vectorized operators, a typed query plan and an
//! EXPLAIN-style plan output.
//!
//! The engine reads a Parquet file into [`Dataset`]s of column chunks and
//! executes a [`QueryPlan`] over them. The pipeline is fixed and pinned:
//!
//! ```text
//! scan -> filter -> project -> aggregate
//! ```
//!
//! and the optimisation order is pinned as well:
//!
//! ```text
//! predicate pushdown -> projection pushdown -> late materialization -> aggregate
//! ```
//!
//! Both orders are stated in [`plan::OPTIMIZATION_ORDER`] and printed in the
//! plan output, so a reader never has to guess what the engine did.
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
//! - **Morsel-driven parallelism.** This milestone is single-threaded: one
//!   plan runs on one thread, end to end. The operators take a `&Dataset` and
//!   return owned values, so the data path is already shaped for a future
//!   morsel loop, but no morsel driver, no task pool and no per-thread
//!   scheduling exist yet.
//! - **A SQL parser.** See above.
//! - **Spilling.** The memory budget is a pre-flight check, not a mechanism:
//!   a plan whose estimated working set exceeds the budget is rejected before
//!   any work is done, rather than being spilled to disk.
//! - **Joins, sorting, and window functions.** The operator set is
//!   filter, project and hash aggregate.
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
pub mod explain;
pub mod memory;
pub mod operators;
pub mod plan;
pub mod pushdown;
pub mod source;

use columnar_core::SelectionVector;
use columnar_kernels::Flavor;

use crate::plan::ColumnSchema;
use crate::pushdown::Pushdown;

pub use dataset::{Dataset, DatasetError};
pub use explain::{PlanOutput, StageStats};
pub use memory::{MemoryBudget, MemoryError};
pub use operators::aggregate::{AggregateResult, AggregateValue, Group};
pub use operators::filter::FilterStats;
pub use plan::{Aggregate, AggregateSpec, QueryPlan};
pub use pushdown::{PushdownMetrics, PushdownPlan};
pub use source::{ParquetSource, SourceColumn};

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
    let resolved = plan.resolve(dataset)?;

    let estimate = memory::estimate_resolved(&resolved, dataset);
    if estimate > options.memory_budget.bytes {
        return Err(EngineError::Memory(MemoryError::BudgetExceeded));
    }

    let scan = StageStats {
        stage: "scan".to_owned(),
        rows_in: rows(dataset.len()),
        rows_out: rows(dataset.len()),
        elapsed: std::time::Duration::ZERO,
    };

    let report = ScanReport {
        pushed_down: Vec::new(),
        columns_read: dataset.column_names(),
        pushdown_metrics: PushdownMetrics::default(),
    };

    run_pipeline(&resolved, dataset, options, scan, report, estimate)
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
/// - **The plan's predicates are pushed into the reader**, so the rows that
///   fail them are dropped during the scan rather than after it.
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
    let resolved = plan.resolve_against(source)?;
    let decision = decide_scan(&resolved, source);

    // The same flavor the filter would use, so the pushdown and the filter
    // run the same kernel over the same values.
    let flavor = options.flavor.unwrap_or_else(columnar_kernels::best);

    let scan_started = std::time::Instant::now();
    let dataset = source.read(&decision.read, &decision.pushdown, flavor)?;
    let scan_elapsed = scan_started.elapsed();

    // Every index the plan holds is a column of the *file*; the dataset holds
    // only the read set, so the indices have to move.
    let mapping = build_mapping(&decision.read, source.column_count());
    let effective = resolved.remap(&mapping);

    // The estimate is checked against the file's row count rather than the
    // surviving rows: the peak of the working set is during the read, when
    // every row of the file is still in flight.
    let estimate = memory::estimate_resolved_at(&effective, &dataset, source.rows());
    if estimate > options.memory_budget.bytes {
        return Err(EngineError::Memory(MemoryError::BudgetExceeded));
    }

    let scan = StageStats {
        stage: "scan".to_owned(),
        rows_in: rows(source.rows()),
        rows_out: rows(dataset.len()),
        elapsed: scan_elapsed,
    };

    let report = ScanReport {
        pushed_down: decision.pushdown_descriptions(),
        columns_read: source.column_names(&fetched_columns(&decision)),
        pushdown_metrics: decision.pushdown.metrics(),
    };

    run_pipeline(&effective, &dataset, options, scan, report, estimate)
}

/// The columns whose bytes were fetched from the file.
///
/// That is the read set -- the columns the plan materialises -- plus the
/// columns the pushed-down predicates read to evaluate themselves. The
/// reader fetches both, and a column outside this list was never touched.
fn fetched_columns(decision: &ScanDecision) -> Vec<usize> {
    let mut fetched = decision.read.clone();
    for predicate in decision.pushdown.predicates() {
        if !fetched.contains(&predicate.column) {
            fetched.push(predicate.column);
        }
    }
    fetched.sort_unstable();
    fetched
}

/// What a scan reads, and what it pushes into the reader.
#[derive(Debug)]
struct ScanDecision {
    /// The file's root indices to read, ascending.
    read: Vec<usize>,
    /// The predicates to push into the reader.
    pushdown: PushdownPlan,
}

impl ScanDecision {
    /// A human-readable description of each pushed predicate.
    fn pushdown_descriptions(&self) -> Vec<String> {
        self.pushdown.predicates().iter().map(Pushdown::describe).collect()
    }
}

/// Decides what a scan reads and what it pushes down.
///
/// The rules, in order:
///
/// 1. The plan's materialised columns are read. That is the late
///    materialisation set: the group-by, the aggregate and the projection.
/// 2. A predicate whose column is **already being read** is declined. The
///    reader would decode that column twice -- once to evaluate the
///    predicate and once to produce the output -- which is slower than not
///    pushing it.
/// 3. A predicate the pushdown cannot evaluate **exactly** is declined. It
///    is never approximated.
/// 4. A declined predicate's column is read, because the engine's filter
///    stage will need it.
fn decide_scan(resolved: &plan::ResolvedPlan, source: &ParquetSource) -> ScanDecision {
    let mut read = resolved.materialised.clone();
    let mut pushdown = PushdownPlan::new();
    let mut pushed_columns: Vec<usize> = Vec::new();

    for filter in &resolved.filter {
        let Some(column) = source.columns().get(filter.column) else {
            continue;
        };
        if read.contains(&filter.column) {
            continue;
        }
        if !pushdown_supports(&column.arrow_type) {
            continue;
        }
        // Two predicates on the same column are one conjunction over that
        // column, and the reader would decode the column once per predicate.
        // They are pushed together.
        match pushdown.for_column_mut(filter.column) {
            Some(existing) => {
                existing.and(filter.predicate.op, filter.predicate.value.clone());
            }
            None => {
                pushdown.push(Pushdown::new(
                    filter.column,
                    &column.name,
                    filter.predicate.op,
                    filter.predicate.value.clone(),
                ));
            }
        }
        pushed_columns.push(filter.column);
    }

    for filter in &resolved.filter {
        if pushed_columns.contains(&filter.column) {
            continue;
        }
        if !read.contains(&filter.column) {
            read.push(filter.column);
        }
    }

    read.sort_unstable();
    read.dedup();

    ScanDecision { read, pushdown }
}

/// Whether the pushdown evaluates this Arrow type exactly.
///
/// The pushdown implements the engine's three physical types. A dictionary
/// column is included because the pushdown compares it once per distinct
/// value and then maps through the indices, exactly as the engine's own
/// dictionary path does.
fn pushdown_supports(arrow_type: &arrow::datatypes::DataType) -> bool {
    use arrow::datatypes::DataType as ArrowType;
    match arrow_type {
        ArrowType::Int64 | ArrowType::Float64 | ArrowType::Utf8 | ArrowType::LargeUtf8 => true,
        ArrowType::Dictionary(key, value) => {
            matches!(key.as_ref(), ArrowType::Int32)
                && matches!(value.as_ref(), ArrowType::Utf8 | ArrowType::LargeUtf8)
        }
        _ => false,
    }
}

/// Maps the file's column indices onto the read set.
///
/// The entry for a column that is not read is `None`.
fn build_mapping(read: &[usize], columns: usize) -> Vec<Option<usize>> {
    (0..columns).map(|index| read.binary_search(&index).ok()).collect()
}

/// What the scan did, for the plan output.
#[derive(Debug, Default)]
struct ScanReport {
    /// The predicates that were pushed into the reader.
    pushed_down: Vec<String>,
    /// The columns that were read from the file.
    columns_read: Vec<String>,
    /// What the pushed-down predicates cost.
    pushdown_metrics: PushdownMetrics,
}

/// Runs the pipeline over an already-resolved plan.
fn run_pipeline(
    resolved: &plan::ResolvedPlan,
    dataset: &Dataset,
    options: &ExecOptions,
    scan: StageStats,
    report: ScanReport,
    estimate: usize,
) -> Result<AggregateResult, EngineError> {
    let flavor = options.flavor.unwrap_or_else(columnar_kernels::best);
    let total_rows = dataset.len();
    let mut stages: Vec<StageStats> = vec![scan];
    let mut materialised =
        operators::project::Materialised::new(dataset, SelectionVector::empty(total_rows));

    // --- filter -----------------------------------------------------------
    // A predicate that was pushed into the reader has already been applied,
    // and its column is not in the dataset. The filters left here are the
    // ones the reader did not take, and the conjunction is separable, so
    // applying them now gives the same answer.
    let selection = if resolved.filter.is_empty() {
        operators::all_rows(total_rows)
    } else {
        let start = std::time::Instant::now();
        // Every predicate is evaluated against the chunk and their masks are
        // ANDed. For a dictionary-encoded column the predicate is evaluated
        // once per distinct value rather than once per row.
        let (selection, stats) = operators::filter::filter_all(dataset, &resolved.filter, flavor)?;
        stages.push(StageStats {
            stage: "filter".to_owned(),
            rows_in: stats.rows_in,
            rows_out: stats.rows_out,
            elapsed: start.elapsed(),
        });
        selection
    };
    materialised.set_selection(selection.clone());

    // --- project ----------------------------------------------------------
    // The project stage materialises exactly the columns the rest of the
    // plan reads. A column that is neither filtered nor aggregated is never
    // decoded: that is the late materialisation the plan output reports.
    let start = std::time::Instant::now();
    for &column in &resolved.materialised {
        materialised.gather(column);
    }
    let selected_rows = materialised.selection().len();
    stages.push(StageStats {
        stage: "project".to_owned(),
        rows_in: rows(selected_rows),
        rows_out: rows(selected_rows),
        elapsed: start.elapsed(),
    });

    // --- aggregate --------------------------------------------------------
    let mut groups: Vec<Group> = Vec::new();
    if let Some(aggregate) = &resolved.aggregate {
        let start = std::time::Instant::now();
        let group_by: Vec<&columnar_core::BatchColumn> =
            aggregate.group_by.iter().map(|&index| materialised.gathered_ref(index)).collect();
        let values: Vec<Option<&columnar_core::BatchColumn>> = aggregate
            .aggregates
            .iter()
            .map(|function| function.column().map(|index| materialised.gathered_ref(index)))
            .collect();
        groups =
            operators::aggregate::aggregate(&aggregate.spec, &group_by, &values, selected_rows);
        stages.push(StageStats {
            stage: "aggregate".to_owned(),
            rows_in: rows(selected_rows),
            rows_out: rows(groups.len()),
            elapsed: start.elapsed(),
        });
    }

    let plan_output = PlanOutput {
        optimization_order: plan::OPTIMIZATION_ORDER,
        stages: stages.clone(),
        materialised_columns: materialised.materialised_names(),
        estimated_bytes: estimate,
        budget_bytes: options.memory_budget.bytes,
        kernel_flavor: operators::flavor_name(flavor).to_owned(),
        pushed_down: report.pushed_down,
        columns_read: report.columns_read,
        pushdown_metrics: report.pushdown_metrics,
    };

    Ok(AggregateResult { groups, stages, plan_output })
}

/// Converts a row count into the `u64` the statistics use.
fn rows(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}
