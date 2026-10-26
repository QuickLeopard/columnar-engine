//! The size curve: one query, at several dataset sizes.
//!
//! ## What this is for
//!
//! The repository already publishes a thread-count sweep and a load sweep.
//! Both of them hold the *data* fixed and move something else. This curve is
//! the third axis: it holds the query, the thread count and the mode fixed,
//! and moves the **size of the data**.
//!
//! It is the axis a reader asks about when they want to know what a query
//! costs *per row*. A table of whole-file times answers "how long did this
//! take" and leaves "and what does that cost per row" open, so the table
//! carries a per-million-rows column beside the timings: the cost per row
//! falls as the data grows, because the fixed costs amortise, and that is
//! exactly what a per-million-rows column shows and a whole-file column
//! hides.
//!
//! ## Why the incumbents are in it
//!
//! A curve with only this engine in it would be a curve of one system's
//! behaviour, and a reader would have to guess what it should be compared
//! against. Principle V requires the comparison to be symmetric: the
//! incumbents run at the same thread count, on the same files, in the same
//! harness, and the curve is published rather than only its endpoint.
//!
//! ## What a number here means
//!
//! Every number is a **cold** number: the file is read and the query is
//! answered in one go, which is what a user waiting on a command line pays.
//! The engine's number is the **lazy** path, because the lazy path is the
//! one that reads the file during the query -- which is exactly what the
//! incumbents' cold numbers do.
//!
//! A system that could not be measured is reported as **missing**, not as a
//! zero. A missing figure must not read as a fast one, and a curve with a
//! hole in it is honest where a curve that aborted is not.

// The row count and the byte count are converted to `f64` for the
// per-million-rows figure and the MiB display; a count beyond 2^53 is not a
// number this report can reach.
#![allow(clippy::cast_precision_loss)]

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::machine::MachineSpec;

/// One point of the size curve: every system, at one size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SizePoint {
    /// The dataset's scale factor, as its manifest records it.
    ///
    /// `None` when the dataset has no manifest -- a file produced by
    /// something other than this repository's generator. It is not zero: a
    /// report that says "scale factor 0" for a dataset it knows nothing
    /// about is not a report.
    pub scale_factor: Option<u32>,
    /// The dataset's rows.
    pub rows: u64,
    /// The dataset's bytes on disk.
    pub bytes: u64,
    /// The engine's cold number: the whole query, the file included.
    pub engine_ms: f64,
    /// The engine's scan stage.
    pub engine_scan_ms: f64,
    /// The engine's operator stages.
    pub engine_operators_ms: f64,
    /// `DuckDB`'s cold number, at the same thread count.
    ///
    /// `None` when it could not be measured.
    pub duckdb_ms: Option<f64>,
    /// `Polars`' cold number.
    ///
    /// `None` when it could not be measured.
    pub polars_ms: Option<f64>,
    /// The `DataFusion` peer's cold number.
    ///
    /// `None` when it could not be measured.
    pub datafusion_ms: Option<f64>,
}

/// The whole curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SizeReport {
    /// The machine the measurement was taken on.
    pub machine: MachineSpec,
    /// The query that was swept.
    pub query: String,
    /// The SQL the query corresponds to.
    pub sql: String,
    /// How the number was measured. Always `cold` here.
    pub mode: String,
    /// The engine's memory budget, in bytes.
    ///
    /// The budget is a policy knob and not a limit on the engine: below it,
    /// it changes nothing at all, and above it the query is refused before
    /// any row is read. The figure is recorded because the curve says which
    /// budget it was measured under.
    pub memory_budget_bytes: u64,
    /// The points of the curve, in the order the files were given.
    pub points: Vec<SizePoint>,
}

impl SizePoint {
    /// The engine's cost per million rows, in milliseconds.
    ///
    /// This is the figure the curve exists for: a reader asking "what does
    /// this cost per row" wants the per-row cost, and a table of whole-file
    /// times makes them divide it out by hand.
    ///
    /// A dataset with no rows has no per-row cost, and `NaN` says so. A zero
    /// would say the query was free, which is a different claim.
    #[must_use]
    pub fn engine_ms_per_million_rows(&self) -> f64 {
        per_million_rows(self.engine_ms, self.rows)
    }
}

impl SizeReport {
    /// Writes the report as pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or written.
    pub fn write_json(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let json = serde_json::to_string_pretty(self).map_err(|error| {
            std::io::Error::other(format!("could not serialise the report: {error}"))
        })?;
        std::fs::write(path, format!("{json}\n"))
    }

    /// Renders the report as a human-readable summary.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write_header(&mut out);
        self.write_table(&mut out);
        out
    }

    /// Writes the header: the machine, the query, and how it was measured.
    fn write_header(&self, out: &mut String) {
        out.push_str("COLUMNAR ENGINE \u{b7} SIZE CURVE\n");
        writeln!(
            out,
            "machine   {} \u{b7} {} \u{b7} {} cores \u{b7} SIMD {}",
            self.machine.cpu, self.machine.arch, self.machine.cores, self.machine.simd
        )
        .ok();
        writeln!(out, "toolchain {}", self.machine.rustc).ok();
        writeln!(out, "os        {}", self.machine.os).ok();
        writeln!(out, "query     {} \u{b7} {}", self.query, self.sql).ok();
        writeln!(
            out,
            "budget    {} MiB of the engine's memory budget",
            self.memory_budget_bytes / (1024 * 1024)
        )
        .ok();
        writeln!(
            out,
            "mode      {} \u{b7} the file is read and the query answered in one go",
            self.mode
        )
        .ok();
        writeln!(out).ok();
    }

    /// Writes the curve itself: one row per size.
    fn write_table(&self, out: &mut String) {
        // The header is built from the same widths as the rows, so the two
        // cannot drift apart.
        let header = format!(
            "{:>5}  {:>10}  {:>7}  {:>8}   {:>11}   {:>10}  {:>9}  {:>9}  {:>10}  {:>8}",
            "sf",
            "rows",
            "MiB",
            "engine",
            "engine-scan",
            "engine-ops",
            "duckdb",
            "polars",
            "datafusion",
            "ms/Mrow"
        );
        writeln!(out, "{header}").ok();
        writeln!(out, "{}", "-".repeat(header.len())).ok();

        for point in &self.points {
            writeln!(
                out,
                "{:>5}  {:>10}  {:>7.1}  {:>8.1}   {:>11.1}   {:>10.1}  {:>9}  {:>9}  {:>10}  {:>8}",
                scale_label(point.scale_factor),
                point.rows,
                point.bytes as f64 / (1024.0 * 1024.0),
                point.engine_ms,
                point.engine_scan_ms,
                point.engine_operators_ms,
                optional_label(point.duckdb_ms),
                optional_label(point.polars_ms),
                optional_label(point.datafusion_ms),
                label(point.engine_ms_per_million_rows()),
            )
            .ok();
        }
    }
}

/// A time, scaled to a million rows.
///
/// The row count is the denominator. A dataset with no rows has no per-row
/// cost, and `NaN` is the honest answer to that.
fn per_million_rows(ms: f64, rows: u64) -> f64 {
    if rows == 0 || !ms.is_finite() {
        return f64::NAN;
    }
    ms / (rows as f64 / 1_000_000.0)
}

/// A figure, or a dash when there is none.
///
/// The dash is not a zero: a system that could not be measured is missing
/// from the curve, and a missing figure must not read as a fast one.
fn label(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.1}")
    } else {
        "-".to_owned()
    }
}

/// A figure that may be missing.
fn optional_label(value: Option<f64>) -> String {
    label(value.unwrap_or(f64::NAN))
}

/// A scale factor, or a dash when the dataset's manifest did not say.
fn scale_label(scale_factor: Option<u32>) -> String {
    scale_factor.map_or_else(|| "-".to_owned(), |scale| scale.to_string())
}

#[cfg(test)]
mod tests {
    use super::{SizePoint, SizeReport};
    use crate::machine::MachineSpec;

    /// The machine to render a report against.
    ///
    /// The real capture asks the OS about itself by running processes, and a
    /// unit test has no business doing that.
    fn machine() -> MachineSpec {
        MachineSpec {
            cpu: "test".to_owned(),
            arch: "arm64".to_owned(),
            cores: 14,
            os: "macOS test".to_owned(),
            simd: "neon".to_owned(),
            forced_simd: "auto".to_owned(),
            rustc: "rustc test".to_owned(),
            threads: 1,
        }
    }

    /// A point with every system measured.
    fn point() -> SizePoint {
        SizePoint {
            scale_factor: Some(1),
            rows: 6_000_000,
            bytes: 1_048_576,
            engine_ms: 100.0,
            engine_scan_ms: 60.0,
            engine_operators_ms: 40.0,
            duckdb_ms: Some(90.0),
            polars_ms: Some(80.0),
            datafusion_ms: Some(70.0),
        }
    }

    /// A report holding the given points.
    fn report(points: Vec<SizePoint>) -> SizeReport {
        SizeReport {
            machine: machine(),
            query: "q6-revenue".to_owned(),
            sql: "SELECT sum(l_extendedprice * (1 - l_discount)) FROM lineitem".to_owned(),
            mode: "cold".to_owned(),
            memory_budget_bytes: 1024 * 1024 * 1024,
            points,
        }
    }

    /// The row a point was rendered as.
    fn row_of(rendered: &str) -> String {
        rendered
            .lines()
            .find(|line| line.contains("6000000"))
            .unwrap_or_else(|| panic!("no row for the point in:\n{rendered}"))
            .to_owned()
    }

    /// The per-million-rows figure is the whole time over the million-row
    /// count: a hundred milliseconds over six million rows is 16.7 ms per
    /// million rows.
    #[test]
    fn per_million_rows_scales_the_whole_time() {
        let per_million = point().engine_ms_per_million_rows();
        assert!((per_million - 100.0 / 6.0).abs() < 1e-9, "was {per_million}");
    }

    /// A dataset with no rows has no per-row cost, and `NaN` says so. A zero
    /// would say the query was free.
    #[test]
    fn per_million_rows_is_undefined_for_an_empty_dataset() {
        let mut point = point();
        point.rows = 0;
        assert!(point.engine_ms_per_million_rows().is_nan());
    }

    /// A system that could not be measured is rendered as a dash, so that a
    /// missing figure cannot be read as a fast one.
    #[test]
    fn a_missing_incumbent_is_rendered_as_a_dash() {
        let mut point = point();
        point.duckdb_ms = None;
        let row = row_of(&report(vec![point]).render());
        assert!(row.contains('-'), "the missing incumbent is not marked: {row}");
        assert!(row.contains("80.0"), "the measured incumbent is missing: {row}");
        assert!(row.contains("70.0"), "the measured incumbent is missing: {row}");
    }

    /// Every measured system is rendered as a number, and the row carries the
    /// per-million-rows figure the curve exists for.
    #[test]
    fn a_measured_incumbent_is_rendered_as_a_number() {
        let row = row_of(&report(vec![point()]).render());
        assert!(!row.contains('-'), "a measured incumbent is marked as missing: {row}");
        assert!(row.contains("16.7"), "the per-million-rows figure is missing: {row}");
    }

    /// A dataset whose manifest did not say has no scale factor, and that is
    /// rendered as a dash rather than as a zero.
    #[test]
    fn an_unknown_scale_factor_is_rendered_as_a_dash() {
        let mut point = point();
        point.scale_factor = None;
        let row = row_of(&report(vec![point]).render());
        assert!(row.starts_with("    -"), "the scale factor is not marked: {row}");
    }
}
