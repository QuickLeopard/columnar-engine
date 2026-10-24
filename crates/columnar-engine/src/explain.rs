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
