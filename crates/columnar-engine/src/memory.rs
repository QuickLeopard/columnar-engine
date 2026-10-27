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
//!
//! ## The lazy path's estimate is a *pre-flight* one
//!
//! The eager path's dataset is already in memory when the plan runs, so its
//! estimate is made over the dataset. The lazy path's dataset does not exist
//! yet, so its estimate is made over the **file**: its schema and its row
//! count. That estimate is computed and checked *before* the read, so a plan
//! that does not fit the budget is refused having read nothing at all.
//!
//! The estimate also accounts for the **read's own peak**, which is where the
//! thread count shows up. A single-threaded read decodes straight into one
//! dataset and holds one copy of it. A parallel read holds every thread's part
//! at once -- which is one copy between them -- and then holds the merged
//! result as well, so it holds **two**. See [`estimate_lazy_read`].

use columnar_core::{DataType, Encoding};

use crate::plan::{QueryPlan, ResolvedPlan};
use crate::source::ParquetSource;
use crate::Dataset;

/// The default memory budget: 256 MiB.
const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// The estimated bytes per aggregate hash-table entry.
const GROUP_ENTRY_BYTES: usize = 16;

/// The estimated bytes per build-side hash-table entry.
///
/// The entry holds the key and the number of build rows that carry it.
const BUILD_ENTRY_BYTES: usize = 16;

/// The estimated bytes per row the sort holds.
const SORT_ENTRY_BYTES: usize = 16;

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
    estimate_resolved_at(resolved, dataset, dataset.len())
}

/// Estimates the working set of an already-resolved plan over `rows` rows.
///
/// The row count is a parameter because the two paths have different peaks.
/// The eager path's peak is the decoded dataset, which holds `dataset.len()`
/// rows. The lazy path's peak is *during* the read, when every row of the
/// file is still in flight even though only some of them survive the
/// pushed-down predicates -- so the lazy path estimates against the file's
/// row count, which is the conservative one.
#[must_use]
pub fn estimate_resolved_at(resolved: &ResolvedPlan, dataset: &Dataset, rows: usize) -> usize {
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

/// Estimates the working set of a **lazy** read of `source`, in bytes.
///
/// This is the pre-flight estimate: it is made from the file's schema and its
/// row count, before a single row is read, so a plan that does not fit the
/// budget is refused having done no work.
///
/// `read` names the file's root columns the read will fetch -- the plan's
/// materialised columns plus the ones the reader needs for a predicate it
/// did not push down. The dataset the read produces holds exactly those
/// columns.
///
/// `threads` is the read's thread count, and it decides the **peak**:
///
/// - **One thread.** The reader decodes straight into one dataset, so the
///   read holds one copy of the columns.
/// - **More than one.** Every thread's part is alive at once -- one copy
///   between them -- and the merge then holds the merged result as well, so
///   the read holds **two** copies. That second copy is the price of the
///   parallel path, and a budget that did not account for it would let a
///   query through that the machine cannot hold.
///
/// The row count used is the **file's**, not the number of rows that survive
/// the pushed-down predicates, because the peak is during the read: every row
/// of the file is still in flight then. The estimate is an upper bound.
#[must_use]
pub fn estimate_lazy_read(
    source: &ParquetSource,
    read: &[usize],
    threads: usize,
    grouped: bool,
) -> usize {
    let rows = source.rows();
    if rows == 0 {
        return 0;
    }

    let mut dataset = 0usize;
    for &index in read {
        let width = source.columns().get(index).map_or(8, |column| type_width(column.data_type));
        dataset =
            dataset.saturating_add(rows.saturating_mul(width).saturating_add(rows.div_ceil(8)));
    }

    let mut bytes = dataset.saturating_mul(read_copies(threads));

    // The filter's match mask and the selection vector it becomes.
    bytes = bytes.saturating_add(rows.div_ceil(8).saturating_mul(2));

    // The aggregate hash table: one entry per group. A **grouped** aggregate
    // can have as many groups as it has rows, so the rows bound it; a
    // **global** aggregate has exactly one, and counting one entry per row
    // would overstate the working set by six million times.
    let groups = if grouped { rows } else { 1 };
    bytes = bytes.saturating_add(groups.saturating_mul(GROUP_ENTRY_BYTES));

    bytes
}

/// How many copies of the read set the read holds at once.
///
/// One thread decodes into one dataset and holds one copy. A parallel read
/// holds every part at once and then the merged result, so it holds two.
fn read_copies(threads: usize) -> usize {
    if threads <= 1 {
        1
    } else {
        2
    }
}

/// The width of one value of a column of the given type.
///
/// A dictionary-encoded string column is a `utf8` column to the engine, and
/// costs the same as one: the dictionary is shared by the whole column, and
/// the per-row cost is the index.
fn type_width(data_type: DataType) -> usize {
    match data_type {
        DataType::Int64 | DataType::Float64 => 8,
        DataType::Utf8 => STRING_HEADER_BYTES,
    }
}

/// Estimates the working set of a join's **build side**, in bytes.
///
/// The build side is hashed: the hash table holds one entry per distinct
/// key, and the build side's row count bounds it. The estimate is made over
/// the file, before a single row of it is read.
#[must_use]
pub fn estimate_build_side(source: &ParquetSource, build_key: &str) -> usize {
    let rows = source.rows();
    if rows == 0 {
        return 0;
    }
    let width = source
        .columns()
        .iter()
        .find(|column| column.name == build_key)
        .map_or(8, |column| type_width(column.data_type));
    rows.saturating_mul(width.saturating_add(BUILD_ENTRY_BYTES))
}

/// Estimates the working set of a **sort**, in bytes.
///
/// With a limit the sort keeps a bounded heap of `limit` rows, so the heap
/// is its working set however many rows it is handed. Without one it sorts
/// every row, and the rows it sorts are its working set.
#[must_use]
pub fn estimate_sort(rows: usize, limit: Option<usize>) -> usize {
    let rows = match limit {
        Some(limit) => limit.min(rows),
        None => rows,
    };
    rows.saturating_mul(SORT_ENTRY_BYTES)
}
