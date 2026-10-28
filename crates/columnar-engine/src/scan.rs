//! The scan's decision: what the scan reads, and what it pushes down.
//!
//! A query over a Parquet file is answered by reading **less** of the file
//! than the file holds. The decision is made here, in one place, from two
//! things: the plan's own **late-materialisation set** -- the columns the
//! query reads -- and the **predicates** the reader can evaluate exactly.
//!
//! The rules, in order:
//!
//! 1. The plan's materialised columns are read. That is the late
//!    materialisation set: the group-by, the aggregate and the projection.
//! 2. A predicate whose column is **already being read** is declined. The
//!    engine's filter stage applies the predicates the scan did not, over the
//!    rows that survived the scan. Pushing this one into the scan would
//!    evaluate it over every row of the file *and* leave the filter to
//!    evaluate it again over the survivors; declining it leaves the work to
//!    the filter, once, over fewer rows.
//! 3. A predicate the pushdown cannot evaluate **exactly** is declined. It
//!    is never approximated.
//! 4. A declined predicate's column is read, because the engine's filter
//!    stage will need it.
//!
//! The decision and the mapping it produces live here together, so that the
//! reader's view of the file -- what it fetched, and where each column went
//! -- is in one place.

use crate::plan::ColumnMapping;
use crate::pushdown::{Pushdown, PushdownMetrics, PushdownPlan};
use crate::source::ParquetSource;

/// The columns whose bytes were fetched from the file.
///
/// That is the read set -- the columns the plan materialises -- plus the
/// columns the pushed-down predicates read to evaluate themselves. The
/// reader fetches both, and a column outside this list was never touched.
pub(crate) fn fetched_columns(decision: &ScanDecision) -> Vec<usize> {
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
pub(crate) struct ScanDecision {
    /// The file's root indices to read, ascending.
    pub(crate) read: Vec<usize>,
    /// The predicates to push into the reader.
    pub(crate) pushdown: PushdownPlan,
    /// Whether **every** predicate was pushed into the reader.
    ///
    /// A plan whose predicates were all pushed has no filter work left, so
    /// the scan decides the rows on its own. That is what feeding the join
    /// needs: the join's probe has to happen before the copy, and the copy
    /// has to happen before the filter.
    pub(crate) fully_pushed: bool,
}

impl ScanDecision {
    /// The mapping from the file's columns onto the read set.
    ///
    /// The read set holds every column the plan materialises, and the
    /// declined predicates' columns beside them, so the mapping holds every
    /// column the plan names.
    pub(crate) fn mapping(&self, columns: usize) -> ColumnMapping {
        ColumnMapping::of(&self.read, columns)
    }

    /// A human-readable description of each pushed predicate.
    pub(crate) fn pushdown_descriptions(&self) -> Vec<String> {
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
///    engine's filter stage applies the predicates the scan did not, over the
///    rows that survived the scan. Pushing this one into the scan would
///    evaluate it over every row of the file *and* leave the filter to
///    evaluate it again over the survivors; declining it leaves the work to
///    the filter, once, over fewer rows.
/// 3. A predicate the pushdown cannot evaluate **exactly** is declined. It
///    is never approximated.
/// 4. A declined predicate's column is read, because the engine's filter
///    stage will need it.
pub(crate) fn decide_scan(
    resolved: &crate::plan::ResolvedPlan,
    source: &ParquetSource,
) -> ScanDecision {
    let mut read = resolved.materialised.clone();
    let mut pushdown = PushdownPlan::new();
    let mut pushed_columns: Vec<usize> = Vec::new();
    let mut fully_pushed = true;

    for filter in &resolved.filter {
        let Some(column) = source.columns().get(filter.column) else {
            fully_pushed = false;
            continue;
        };
        if read.contains(&filter.column) {
            fully_pushed = false;
            continue;
        }
        if !pushdown_supports(&column.arrow_type) {
            fully_pushed = false;
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

    ScanDecision { read, pushdown, fully_pushed }
}

/// Whether the pushdown evaluates this Arrow type exactly.
///
/// The pushdown implements the engine's three physical types. A dictionary
/// column is included because the pushdown compares it once per distinct
/// value and then maps through the indices, exactly as the engine's own
/// dictionary path does.
pub(crate) fn pushdown_supports(arrow_type: &arrow::datatypes::DataType) -> bool {
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
