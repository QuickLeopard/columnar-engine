//! The thread-count sweep.
//!
//! ## What this is for
//!
//! The headline comparison in [`crate::result::BenchReport`] is **pinned to
//! one thread** on both sides. That is the like-for-like claim, and it is
//! unchanged by this module. But a pinned comparison answers one question --
//! "which is faster when both are given one thread" -- and leaves another
//! one open: "how does each system's number move with the thread count?"
//!
//! So this module publishes a **second** table. The engine and both
//! incumbents are run at the *same* thread counts, on the *same* file, in the
//! *same* harness, and the curve is published rather than only its endpoint.
//!
//! ## Why the incumbents are in it
//!
//! A scaling table with only this engine in it would be a table of one
//! system's curve, and a reader would have to guess what it should be
//! compared against. Principle V requires the comparison to be symmetric: the
//! incumbents run at the same thread count as the engine, at every point of
//! the sweep. So the table is a comparison at every thread count, not a
//! measurement with a comparison attached.
//!
//! ## What a number here means
//!
//! Every number is a **cold** number: the file is read and the query is
//! answered in one go, which is what a user waiting on a command line pays.
//! The engine's number is the **lazy** path, because the lazy path is the
//! one that reads the file during the query -- which is exactly what the
//! incumbents' cold numbers do.
//!
//! The engine's *scan* time is reported next to it, because the scan is
//! where the parallelism is, and a reader who sees the total fall while the
//! scan does not knows immediately that the parallelism went somewhere other
//! than where they hoped.

// The byte count is converted to `f64` for the MiB display; a byte count
// beyond 2^53 is not a number this report can reach.
#![allow(clippy::cast_precision_loss)]

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::machine::MachineSpec;
use crate::result::DatasetReport;

/// One point of the sweep: every system, at one thread count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScalingPoint {
    /// The thread count every system ran at.
    pub threads: usize,
    /// How many threads the engine's read **actually** used.
    ///
    /// Not the count that was asked for: the read splits the file's row
    /// groups across the threads, so a file with six row groups has six
    /// workers however many threads are requested. The incumbents are not
    /// capped this way -- `DuckDB` will happily use fourteen threads on a
    /// six-row-group file -- so this column is the one that explains the
    /// shape of the engine's curve.
    pub workers: usize,
    /// The engine's cold number: the whole query, the file included.
    pub engine_ms: f64,
    /// The engine's scan stage: the read, the pushdown and the merge.
    pub engine_scan_ms: f64,
    /// `DuckDB`'s cold number, at the same thread count.
    pub duckdb_ms: f64,
    /// `Polars`' cold number, at the same thread count.
    pub polars_ms: f64,
}

/// The whole sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScalingReport {
    /// The machine the measurement was taken on.
    pub machine: MachineSpec,
    /// The dataset the measurement used.
    pub dataset: DatasetReport,
    /// The query that was swept.
    pub query: String,
    /// The SQL the query corresponds to.
    pub sql: String,
    /// How the number was measured: `cold`, always.
    pub mode: String,
    /// How many row groups the file has.
    ///
    /// This is the **cap** on the engine's parallelism: the read splits the
    /// file's row groups across the threads, so a file with six row groups
    /// cannot be read by more than six threads at a time, however many cores
    /// the machine has. A reader looking at a flat curve past this point
    /// should know that the ceiling is the file's, not the engine's.
    pub row_groups: usize,
    /// The points of the sweep, in ascending thread order.
    pub points: Vec<ScalingPoint>,
}

impl ScalingReport {
    /// Writes the report as pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or written.
    pub fn write_json(&self, path: &std::path::Path) -> std::io::Result<()> {
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

    /// The point with the fewest threads, which is the baseline the others
    /// are scaled against.
    #[must_use]
    pub fn base(&self) -> Option<&ScalingPoint> {
        self.points.iter().min_by_key(|point| point.threads)
    }

    /// Renders the report as a human-readable summary.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("COLUMNAR ENGINE \u{b7} THREAD-COUNT SWEEP\n");
        writeln!(
            out,
            "machine   {} \u{b7} {} \u{b7} {} cores \u{b7} SIMD {}",
            self.machine.cpu, self.machine.arch, self.machine.cores, self.machine.simd
        )
        .ok();
        writeln!(out, "toolchain {}", self.machine.rustc).ok();
        writeln!(out, "os        {}", self.machine.os).ok();
        writeln!(
            out,
            "dataset   {} \u{b7} {} rows \u{b7} {:.1} MiB",
            self.dataset.path,
            self.dataset.rows,
            self.dataset.bytes as f64 / (1024.0 * 1024.0)
        )
        .ok();
        writeln!(out, "query     {} \u{b7} {}", self.query, self.sql).ok();
        writeln!(
            out,
            "mode      {} \u{b7} the file is read and the query answered in one go",
            self.mode
        )
        .ok();
        writeln!(
            out,
            "row groups {} \u{b7} the read splits the row groups across the threads, so this caps the engine's parallelism",
            self.row_groups
        )
        .ok();
        writeln!(out).ok();

        writeln!(
            out,
            "threads   workers     engine   engine-scan     duckdb     polars   engine x    duckdb x"
        )
        .ok();
        writeln!(out, "---------------------------------------------------------------").ok();

        let base = self.base();
        for point in &self.points {
            let engine_speedup = ratio(base.map(|base| base.engine_ms), point.engine_ms);
            let duckdb_speedup = ratio(base.map(|base| base.duckdb_ms), point.duckdb_ms);
            writeln!(
                out,
                "{:>7}  {:>7}  {:>8.1}   {:>11.1}   {:>8.1}   {:>8.1}   {:>8.2}x   {:>7.2}x",
                point.threads,
                point.workers,
                point.engine_ms,
                point.engine_scan_ms,
                point.duckdb_ms,
                point.polars_ms,
                engine_speedup,
                duckdb_speedup
            )
            .ok();
        }

        out
    }
}

/// How many times faster `current` is than `base`.
///
/// A missing or non-finite base is reported as `1.00x` rather than as an
/// infinity: a table with an infinity in it is a table nobody reads.
fn ratio(base: Option<f64>, current: f64) -> f64 {
    match base {
        Some(base) if base.is_finite() && base > 0.0 && current.is_finite() => base / current,
        _ => 1.0,
    }
}
