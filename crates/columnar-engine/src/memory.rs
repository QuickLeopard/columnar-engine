//! The query memory budget.
//!
//! The engine estimates the working set of a plan before it runs it, and
//! refuses the plan if the estimate does not fit the budget. The check is a
//! *pre-flight* check, not a mechanism: a plan that does not fit is
//! rejected, not spilled. Spilling is explicitly out of scope for this
//! milestone (see the crate documentation).
//!
//! ## What the estimate counts
//!
//! | Component | Bytes |
//! |---|---|
//! | a materialised column | `rows * (value width + 1 validity bit)` |
//! | the filter's match mask | `rows / 8` |
//! | the selection vector | `rows / 8` |
//! | the aggregate hash table | `rows * 16` |
//!
//! The hash table's real size depends on the number of groups, which cannot
//! be known before the query runs, so the row count is used as the upper
//! bound. The value width is 8 bytes for `int64` and `float64`, and 24 bytes
//! -- the size of a `String` header -- for `utf8`; the bytes a string's heap
//! holds are not counted, because they are the data's own size rather than
//! the working set's.
//!
//! The estimate is an upper bound, not a prediction. It is meant to fail
//! early and loudly, not to be accurate to the byte.

use columnar_core::Encoding;

use crate::plan::{QueryPlan, ResolvedPlan};
use crate::Dataset;

/// The default memory budget: 256 MiB.
const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// The estimated bytes per aggregate hash-table entry.
const GROUP_ENTRY_BYTES: usize = 16;

/// The width of a `String` header, the value width of a `utf8` column.
const STRING_HEADER_BYTES: usize = 24;

/// The per-query memory budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    /// The budget, in bytes.
    pub bytes: usize,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self { bytes: DEFAULT_BUDGET_BYTES }
    }
}

/// Why a plan did not fit its memory budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    /// The plan's estimated working set does not fit the budget.
    ///
    /// The plan is rejected before any work is done, so a plan that fails
    /// this check has read no rows.
    #[error("the estimated working set exceeds the query memory budget")]
    BudgetExceeded,
}

/// Estimates the working set of `plan` over `dataset`, in bytes.
///
/// A plan that does not resolve (an unknown column, say) has no working set
/// to estimate, and the estimate is zero: [`crate::execute`] reports the
/// resolution error long before it looks at the budget.
#[must_use]
pub fn estimate_bytes(plan: &QueryPlan, dataset: &Dataset) -> usize {
    match plan.resolve(dataset) {
        Ok(resolved) => estimate_resolved(&resolved, dataset),
        Err(_) => 0,
    }
}

/// Estimates the working set of an already-resolved plan, in bytes.
#[must_use]
pub fn estimate_resolved(resolved: &ResolvedPlan, dataset: &Dataset) -> usize {
    let rows = dataset.len();
    if rows == 0 {
        return 0;
    }

    let mut bytes = 0usize;
    for &index in &resolved.materialised {
        let chunk = dataset.chunks.get(index).expect("a materialised column index is in range");
        bytes = bytes.saturating_add(column_bytes(chunk, rows));
    }

    // The filter's match mask and the selection vector it becomes.
    bytes = bytes.saturating_add(rows.div_ceil(8).saturating_mul(2));

    // The aggregate hash table: one entry per group, bounded by the rows.
    bytes = bytes.saturating_add(rows.saturating_mul(GROUP_ENTRY_BYTES));

    bytes
}

/// The bytes a materialised column costs.
fn column_bytes(chunk: &columnar_core::ColumnChunk, rows: usize) -> usize {
    let value_width = match &chunk.encoding {
        Encoding::Int64 { .. }
        | Encoding::Float64 { .. }
        | Encoding::Int64Dict { .. }
        | Encoding::Float64Dict { .. }
        | Encoding::BoolRle { .. } => 8,
        Encoding::Utf8 { .. } | Encoding::Utf8Dict { .. } => STRING_HEADER_BYTES,
    };
    let validity = rows.div_ceil(8);
    rows.saturating_mul(value_width).saturating_add(validity)
}
