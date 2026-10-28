//! The scan's decision: what the scan reads, and what it pushes down.
//!
//! A query over a Parquet file is answered by reading **less** of the file
//! than the file holds. The decision is made by the **reader** --
//! [`ParquetSource::decide_read`] -- from two things: the plan's own
//! **late-materialisation set** -- the columns the query reads -- and the
//! **predicates** the reader can evaluate exactly.
//!
//! The reader's rules, in order:
//!
//! 1. The plan's materialised columns are read. That is the late
//!    materialisation set: the group-by, the aggregate and the projection.
//! 2. A predicate whose column is **already being read** is declined. The
//!    engine's filter stage applies the predicates the reader did not, over
//!    the rows that survived the scan. Pushing this one into the scan would
//!    evaluate it over every row of the file *and* leave the filter to
//!    evaluate it again over the survivors; declining it leaves the work to
//!    the filter, once, over fewer rows.
//! 3. A predicate the pushdown cannot evaluate **exactly** is declined. It
//!    is never approximated.
//! 4. A declined predicate's column is read, because the engine's filter
//!    stage will need it.
//!
//! **The rules live in one place, and every read asks them.** The probe side's
//! scan asks the reader with the plan's materialised set and the plan's
//! predicates; the join's build side asks the same reader with its own key as
//! its materialised set and its own predicates. Two copies of these rules
//! could disagree, and the disagreement would show up as a plan that reads a
//! different set of columns on each side of a join.
//!
//! What is left here is the scan's **own** rule: whether the scan can feed
//! the join, which is a question about the join and not about the read.

use crate::operators::key::ArrowKeys;
use crate::plan::ResolvedPlan;
use crate::pushdown::{Pushdown, PushdownMetrics};
use crate::source::{scan_projection, ParquetSource, ReadDecision};
use crate::EngineError;

/// The columns whose bytes were fetched from the file.
///
/// That is the read set -- the columns the plan materialises -- plus the
/// columns the pushed-down predicates read to evaluate themselves. The
/// reader fetches both, and a column outside this list was never touched.
pub(crate) fn fetched_columns(decision: &ReadDecision) -> Vec<usize> {
    let mut fetched = decision.read.clone();
    for predicate in decision.pushdown.predicates() {
        if !fetched.contains(&predicate.column) {
            fetched.push(predicate.column);
        }
    }
    fetched.sort_unstable();
    fetched
}

/// A human-readable description of each pushed predicate.
pub(crate) fn pushdown_descriptions(decision: &ReadDecision) -> Vec<String> {
    decision.pushdown.predicates().iter().map(Pushdown::describe).collect()
}

/// Decides what the scan reads and what it pushes down.
///
/// The decision is the reader's, and the scan asks for it: the plan's
/// materialised set is what the plan reads, and the plan's predicates are
/// what the reader may take.
///
/// # Errors
///
/// Returns [`EngineError::UnknownColumn`] when a predicate names a column the
/// file does not have.
pub(crate) fn decide_scan(
    resolved: &ResolvedPlan,
    source: &ParquetSource,
) -> Result<ReadDecision, EngineError> {
    source.decide_read(&resolved.materialised, &resolved.filter)
}

/// The join's key column in the reader's projection, when the scan can feed
/// the join.
///
/// The scan feeds the join when the join's probe can happen **before** the
/// copy: the plan has a join, every predicate was pushed into the reader --
/// so the filter stage has no work to do -- and the join's key is a type the
/// probe reads. See
/// [ADR-0030](../../../docs/adr/0030-the-scan-that-feeds-the-join.md).
pub(crate) fn streamed_join(
    resolved: &ResolvedPlan,
    decision: &ReadDecision,
    source: &ParquetSource,
) -> Option<usize> {
    let join = resolved.join.as_ref()?;
    if !decision.fully_pushed {
        return None;
    }
    let column = source.columns().get(join.probe_key)?;
    // The **probe's** own list of the types it can read. The pushdown has a
    // list of its own -- what the reader can evaluate exactly -- and the two
    // are not the same question.
    if !ArrowKeys::supports(&column.arrow_type) {
        return None;
    }
    let projection = scan_projection(&decision.read, &decision.pushdown);
    projection.iter().position(|&index| index == join.probe_key)
}

/// What the scan did, for the plan output.
#[derive(Debug, Default)]
pub(crate) struct ScanReport {
    /// The predicates that were pushed into the reader.
    pub(crate) pushed_down: Vec<String>,
    /// The columns that were read from the file.
    pub(crate) columns_read: Vec<String>,
    /// What the pushed-down predicates cost.
    pub(crate) pushdown_metrics: PushdownMetrics,
}
