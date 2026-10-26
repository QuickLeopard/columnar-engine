//! The EXPLAIN-style plan output.
//!
//! [`PlanOutput`] is the rendered plan: the pinned optimisation order, one
//! line per stage, the columns that were actually materialised, the memory
//! estimate against the budget, and the kernel flavor that ran.
//!
//! The output is a *report about the execution*, not a prediction: the stage
//! timings are measured, the materialised column list is what the executor
//! really touched, and the row counts are what really flowed through.

use std::fmt;

use crate::plan::OPTIMIZATION_ORDER;
use crate::pushdown::PushdownMetrics;

/// Per-stage execution statistics, in execution order.
#[derive(Debug, Clone, PartialEq)]
pub struct StageStats {
    /// The stage name, e.g. "scan", "filter", "project", "aggregate".
    pub stage: String,
    /// Rows entering the stage.
    pub rows_in: u64,
    /// Rows leaving the stage.
    pub rows_out: u64,
    /// Wall-clock time spent in the stage.
    pub elapsed: std::time::Duration,
}

/// The rendered plan.
///
/// Render it with [`fmt::Display`], or with
/// [`std::fmt::Write::write_fmt`] into any formatter.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanOutput {
    /// The pinned optimisation order the engine applied.
    pub optimization_order: &'static str,
    /// The stages, in execution order.
    pub stages: Vec<StageStats>,
    /// The columns that were decoded, in schema order.
    ///
    /// This is the late-materialisation report: a column that is neither
    /// filtered nor aggregated is absent from this list, because it was
    /// never decoded.
    pub materialised_columns: Vec<String>,
    /// The estimated working set, in bytes.
    pub estimated_bytes: usize,
    /// The memory budget the estimate was checked against, in bytes.
    pub budget_bytes: usize,
    /// The kernel flavor that ran the filter.
    pub kernel_flavor: String,
    /// The predicates that were pushed into the scan.
    ///
    /// A predicate that was pushed was applied by the reader, during the
    /// scan, so the rows that failed it never reached the operators behind
    /// it. The list is empty for the eager path, which pushes nothing.
    pub pushed_down: Vec<String>,
    /// The columns that were read from the file.
    ///
    /// This is the projection report: a column that is neither filtered nor
    /// aggregated is absent from this list, because its bytes were never
    /// fetched.
    pub columns_read: Vec<String>,
    /// How many threads the read used.
    ///
    /// A number measured with one thread and a number measured with eight
    /// are not comparable, and a reader should not have to guess which one
    /// they are looking at.
    pub threads: usize,
    /// How many workers the filter used.
    ///
    /// The filter splits the rows into contiguous blocks, one per worker, so
    /// the number of workers that ran is the number of blocks. It is **not**
    /// the thread count that was asked for: a file with a hundred rows has one
    /// block, however many threads were offered.
    pub filter_workers: usize,
    /// How many workers the projection used.
    ///
    /// The projection splits the *source rows* into blocks, one per worker,
    /// and gathers the survivors in each. It has its own block size, so its
    /// worker count can differ from the filter's, and the two are reported
    /// separately rather than as one number that is true of neither.
    pub projection_workers: usize,
    /// What the pushed-down predicates cost.
    ///
    /// The pushdown runs inside the scan, so its cost is otherwise folded
    /// into the scan stage. This separates the two halves of the scan: the
    /// reader's decode, and the comparison. They have different fixes.
    pub pushdown_metrics: PushdownMetrics,
}

impl fmt::Display for PlanOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "optimization order: {}",
            if self.optimization_order.is_empty() {
                OPTIMIZATION_ORDER
            } else {
                self.optimization_order
            }
        )?;

        let headers = ["stage", "rows in", "rows out", "elapsed"];
        let mut rows: Vec<[String; 4]> = Vec::with_capacity(self.stages.len());
        for stage in &self.stages {
            rows.push([
                stage.stage.clone(),
                stage.rows_in.to_string(),
                stage.rows_out.to_string(),
                format!("{:.1} ms", stage.elapsed.as_secs_f64() * 1000.0),
            ]);
        }

        let mut widths = [0usize; 4];
        for (index, header) in headers.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = header.len();
            }
        }
        for row in &rows {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(cell.len());
                }
            }
        }

        write_row(f, &headers.map(String::from), &widths)?;
        for row in &rows {
            write_row(f, row, &widths)?;
        }

        writeln!(
            f,
            "materialised columns: {}",
            if self.materialised_columns.is_empty() {
                "(none)".to_owned()
            } else {
                self.materialised_columns.join(", ")
            }
        )?;
        writeln!(f, "read threads: {}", self.threads)?;
        writeln!(
            f,
            "operator workers: {} filter, {} projection",
            self.filter_workers, self.projection_workers
        )?;
        writeln!(
            f,
            "columns read: {}",
            if self.columns_read.is_empty() {
                "(none)".to_owned()
            } else {
                self.columns_read.join(", ")
            }
        )?;
        writeln!(
            f,
            "pushed down: {}",
            if self.pushed_down.is_empty() {
                "(none)".to_owned()
            } else {
                self.pushed_down.join(", ")
            }
        )?;
        if self.pushdown_metrics.rows_evaluated > 0 {
            let ps = self.pushdown_metrics.ps_per_row();
            writeln!(
                f,
                "pushdown cost: {:.1} ms over {} rows ({}.{:02} ps/row)",
                self.pushdown_metrics.predicate_time().as_secs_f64() * 1000.0,
                self.pushdown_metrics.rows_evaluated,
                ps / 1000,
                ps % 1000
            )?;
        }
        writeln!(
            f,
            "estimated memory: {} (budget {})",
            format_bytes(self.estimated_bytes),
            format_bytes(self.budget_bytes)
        )?;
        write!(f, "kernel flavor: {}", self.kernel_flavor)
    }
}

/// Writes one row of the stage table.
fn write_row(f: &mut fmt::Formatter<'_>, cells: &[String; 4], widths: &[usize; 4]) -> fmt::Result {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            write!(f, "  ")?;
        }
        let width = widths.get(index).copied().unwrap_or(0);
        if index + 1 == cells.len() {
            // The last cell is not padded, so a line has no trailing
            // whitespace for a diff to trip over.
            write!(f, "{cell}")?;
        } else {
            write!(f, "{cell:<width$}")?;
        }
    }
    writeln!(f)
}

/// Formats a byte count the way a human reads it.
// Byte counts below 2^53 are exact in an f64, and a byte count above that
// is larger than any disk this engine will ever read.
#[allow(clippy::cast_precision_loss)]
fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}
