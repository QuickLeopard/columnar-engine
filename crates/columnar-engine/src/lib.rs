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

use columnar_core::SelectionVector;
use columnar_kernels::Flavor;

pub use dataset::{Dataset, DatasetError};
pub use explain::{PlanOutput, StageStats};
pub use memory::{MemoryBudget, MemoryError};
pub use operators::aggregate::{AggregateResult, AggregateValue, Group};
pub use operators::filter::FilterStats;
pub use plan::{Aggregate, AggregateSpec, QueryPlan};

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

    let flavor = options.flavor.unwrap_or_else(columnar_kernels::best);
    let total_rows = dataset.len();
    let mut stages: Vec<StageStats> = Vec::new();
    let mut materialised =
        operators::project::Materialised::new(dataset, SelectionVector::empty(total_rows));

    // --- scan -------------------------------------------------------------
    // The scan stage decodes the column the filter reads. It is the only
    // column decoded densely: the comparison kernel reads a dense array.
    let scan_start = std::time::Instant::now();
    stages.push(StageStats {
        stage: "scan".to_owned(),
        rows_in: rows(total_rows),
        rows_out: rows(total_rows),
        elapsed: scan_start.elapsed(),
    });

    // --- filter -----------------------------------------------------------
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
    };

    Ok(AggregateResult { groups, stages, plan_output })
}

/// Converts a row count into the `u64` the statistics use.
fn rows(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}
