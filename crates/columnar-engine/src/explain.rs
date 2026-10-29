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

impl StageStats {
    /// A stage's report: what it was handed, what it produced, and how long it
    /// took.
    ///
    /// This is the **one** way a stage reports its work. The stage's name, the
    /// rows entering it, the rows leaving it, and its own time -- the same
    /// four things, in the same order, for every stage the engine runs.
    pub fn new(stage: &str, rows_in: u64, rows_out: u64, elapsed: std::time::Duration) -> Self {
        Self { stage: stage.to_owned(), rows_in, rows_out, elapsed }
    }
}

/// What the join did, for the plan output.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinReport {
    /// The probe side's key column -- the left table's.
    pub probe_key: String,
    /// The build side's key column -- the right table's.
    pub build_key: String,
    /// The build side's file name.
    ///
    /// The name, not the path: the plan output says which file the build
    /// side was, and a reader who wants the path has the plan.
    pub build_file: String,
    /// How many rows the build side held after its own filter.
    pub build_rows: usize,
    /// How many rows the join produced.
    pub output_rows: usize,
    /// Whether the probe ran **inside** the scan.
    ///
    /// When the scan feeds the join, the probe happens before the copy, and
    /// the plan output says so: the scan's elapsed does **not** carry the
    /// probe's cost -- the probe's time is taken out of the scan's -- and the
    /// join's stage reports the build table's construction and the probe
    /// together.
    pub probe_in_scan: bool,
    /// The build table's construction.
    ///
    /// The join's stage is this, the probe, and the build side's file open,
    /// together. They are reported apart because **the stage's cost and the
    /// probe's cost are different questions**: a win on one is not a win on
    /// the other, and the stage's total cannot say which one was won.
    pub build_elapsed: std::time::Duration,
    /// The probe.
    ///
    /// On the cold path this is the probe inside the reader, and on the
    /// resident path it is the probe over the dataset's own column. The two
    /// are the same work over the same rows, measured over the same rows, so
    /// they are comparable.
    pub probe_elapsed: std::time::Duration,
    /// The build side's file open.
    ///
    /// The build side's file is opened before the pipeline runs, and the open
    /// is the join's own work, so the stage carries it.
    pub open_elapsed: std::time::Duration,
}

/// What the sort did, for the plan output.
#[derive(Debug, Clone, PartialEq)]
pub struct SortReport {
    /// The column the rows were sorted by.
    pub column: String,
    /// Whether the order was descending.
    pub descending: bool,
    /// How many rows the sort was asked to keep. `None` means all of them.
    pub limit: Option<usize>,
    /// How many rows the sort kept.
    pub rows: usize,
}

/// How the query's own time divides up.
///
/// The plan output is the engine's own account of its work, so it accounts
/// for the **whole** query: the time the engine measured around the query,
/// the time its stages account for, and the time that fell outside them.
///
/// The last number is the one that matters. A stage table that adds up to
/// less than the query is an account with a hole in it, and the hole is
/// where the engine's work goes to hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attribution {
    /// The whole query's time, as the engine measured it.
    pub elapsed: std::time::Duration,
    /// The time the plan output's stages account for.
    pub accounted: std::time::Duration,
    /// The time that fell outside the stages.
    pub outside: std::time::Duration,
}

impl Attribution {
    /// The attribution of a query that took `elapsed` and whose stages took
    /// `stages`.
    #[must_use]
    pub fn of(elapsed: std::time::Duration, stages: &[StageStats]) -> Self {
        let accounted = stages.iter().map(|stage| stage.elapsed).sum();
        Self { elapsed, accounted, outside: elapsed.saturating_sub(accounted) }
    }
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
    /// The columns the query read, in schema order.
    ///
    /// This is the late-materialisation report: a column that is neither
    /// filtered nor aggregated is absent from this list, because it was
    /// never read.
    ///
    /// A column the aggregate folded **in place**, over the filter's
    /// selection, is in this list — it was read — even though it was never
    /// copied into a column of the survivors. A column the query read is a
    /// column the query read, whether or not the engine materialised it.
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
    /// The join, if the plan had one.
    ///
    /// The report names the two key columns and the build side's file, and
    /// counts the rows the build side held and the rows the join produced.
    pub join: Option<JoinReport>,
    /// The sort, if the plan had one.
    ///
    /// The report names the column the rows were sorted by, the direction,
    /// and the limit the sort was given.
    pub sort: Option<SortReport>,
    /// How the query's own time divides up.
    ///
    /// The stages account for the query's time, and this says so: the
    /// query's own elapsed time, the time the stages account for, and the
    /// time that fell outside them.
    pub attribution: Attribution,
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

        // The account of the whole query: the stages add up to this, and
        // whatever is left over is named here rather than hidden.
        writeln!(
            f,
            "accounted: {:.2} ms of the query's {:.2} ms ({:.2} ms outside the stages)",
            ms(self.attribution.accounted),
            ms(self.attribution.elapsed),
            ms(self.attribution.outside),
        )?;

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
        self.write_join_and_sort(f)?;
        write!(f, "kernel flavor: {}", self.kernel_flavor)
    }
}

impl PlanOutput {
    /// Writes the join's and the sort's reports.
    ///
    /// The join names its two key columns and the build side's file; the sort
    /// names its column, its direction and its limit. A plan with neither
    /// stage prints neither line.
    fn write_join_and_sort(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(join) = &self.join {
            writeln!(
                f,
                "join: equi-join on {} = {}, build side {} ({} rows), {} rows out",
                join.probe_key, join.build_key, join.build_file, join.build_rows, join.output_rows
            )?;
            if join.probe_in_scan {
                writeln!(
                    f,
                    "the probe ran inside the scan: the rows it did not match were never copied"
                )?;
            }
        }
        if let Some(sort) = &self.sort {
            let direction = if sort.descending { "DESC" } else { "ASC" };
            match sort.limit {
                Some(limit) => writeln!(
                    f,
                    "sort: {} {direction} LIMIT {limit} ({} rows kept)",
                    sort.column, sort.rows
                )?,
                None => writeln!(f, "sort: {} {direction} ({} rows)", sort.column, sort.rows)?,
            }
        }
        Ok(())
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

/// A duration, in milliseconds.
fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Formats a byte count the way a human reads it.// Byte counts below 2^53 are exact in an f64, and a byte count above that
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
