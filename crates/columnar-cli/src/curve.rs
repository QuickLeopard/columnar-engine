//! The size curve: the command that measures it.
//!
//! The repository publishes a thread-count sweep and a load sweep. Both hold
//! the *data* fixed and move something else. This one holds the query, the
//! thread count and the mode fixed, and moves the **size of the data** -- the
//! axis a reader asks about when they want to know what a query costs *per
//! row*.
//!
//! ## What it measures
//!
//! For every file it is given, in the order it is given:
//!
//! - **the engine**, cold, through the lazy path: the file is read during
//!   the query, which is what a user waiting on a command line pays;
//! - **the engine's scan stage and its operator stages**, so a reader who
//!   sees the total move knows which half of it moved;
//! - **the three incumbents** -- `DuckDB`, `Polars` and the `DataFusion`
//!   peer -- cold, at the same thread count, on the same file.
//!
//! ## The two rules that make it a comparison
//!
//! - **Every system runs at the same thread count, on the same file, in the
//!   same harness.** Principle V requires the comparison to be symmetric: the
//!   incumbents are pinned to the engine's thread count at every size, the
//!   way the headline comparison pins them to one.
//! - **Every system's answer is cross-checked against the engine's.** A
//!   curve that published timings for systems computing different answers
//!   would be a curve of nothing.
//!
//! ## What it does when something is missing
//!
//! An incumbent whose binary is not installed leaves a **hole** in the curve
//! rather than stopping it. A hole is honest: the reader can see that the
//! system was not measured. An abort is not, and neither is a zero -- a
//! missing figure must not read as a fast one.
//!
//! A disagreement between the engine's answer and an incumbent's is a
//! **warning**, not an abort, for the same reason.

// The engine's integer answers are widened to `f64` for the cross-check
// against the incumbents. The values are counts and revenue sums, so the
// conversion keeps every digit that matters.
#![allow(clippy::cast_precision_loss)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use columnar_bench::external::{self, Comparator, ExternalQuery, Mode};
use columnar_bench::machine::MachineSpec;
use columnar_bench::size::{SizePoint, SizeReport};
use columnar_cli::queries::{sql_for, QueryDefinition};

/// The relative tolerance the answers are cross-checked at.
///
/// A `SUM` over hundreds of thousands of values is not associative in
/// floating point, so two engines that sum in a different order can differ in
/// the last bits. The tolerance is relative, and loose enough for that and
/// tight enough to catch a real disagreement.
const ANSWER_TOLERANCE: f64 = 1e-9;

/// Runs the size curve and writes the report.
///
/// Every file is measured at the same thread count, on the same query, cold.
/// The engine and the three incumbents are measured at every size, and every
/// incumbent's answer is cross-checked against the engine's.
///
/// An incumbent that cannot be measured leaves a hole in the curve rather
/// than stopping it, and a disagreement between two systems' answers is a
/// warning rather than an abort.
///
/// # Errors
///
/// Returns an error if the engine could not run the query on one of the
/// files, or if the report could not be written.
// The crate root dispatches to this, so the item is reachable from where it
// is used; the lint cannot see that through a private module of a binary.
#[allow(unreachable_pub)]
pub fn run(
    data: &[PathBuf],
    definition: &QueryDefinition,
    threads: usize,
    iterations: usize,
    budget: columnar_engine::MemoryBudget,
    out: &Path,
) -> Result<()> {
    let mut points = Vec::with_capacity(data.len());

    for path in data {
        let point = measure_point(definition, path, threads, iterations, budget)?;
        println!(
            "{path} \u{b7} {rows} rows \u{b7} engine {engine:.1} ms \u{b7} duckdb {duckdb} \u{b7} polars {polars} \u{b7} datafusion {datafusion}",
            path = path.display(),
            rows = point.rows,
            engine = point.engine_ms,
            duckdb = figure(point.duckdb_ms),
            polars = figure(point.polars_ms),
            datafusion = figure(point.datafusion_ms),
        );
        points.push(point);
    }

    let report = SizeReport {
        machine: MachineSpec::capture(threads),
        query: definition.query.name().to_owned(),
        sql: definition.sql_for_display(),
        mode: Mode::Cold.name().to_owned(),
        memory_budget_bytes: budget.bytes as u64,
        points,
    };

    report
        .write_json(out)
        .with_context(|| format!("could not write the report to {}", out.display()))?;
    println!();
    println!("{}", report.render());
    println!("report written to {}", out.display());
    Ok(())
}

/// Measures every system at one dataset size.
///
/// The engine goes first: its answer is the reference every incumbent is
/// cross-checked against.
fn measure_point(
    definition: &QueryDefinition,
    path: &Path,
    threads: usize,
    iterations: usize,
    budget: columnar_engine::MemoryBudget,
) -> Result<SizePoint> {
    // The lazy path reads the file *during* the query, so its number is a
    // cold number by construction -- which is what the incumbents' cold
    // numbers are.
    //
    // The headline figure is the median over the runs; the two stage columns
    // come from the last run, so they add up to that run's query rather than
    // to the median.
    let (_open_ms, engine_ms, _median_scan_ms, result) =
        crate::measure_engine_lazy_at(definition, path, iterations, threads, budget)
            .with_context(|| format!("the engine could not run the query on {}", path.display()))?;
    let engine_scan_ms = crate::scan_stage_ms(&result);
    let engine_operators_ms = crate::operator_stage_ms(&result);

    let (duckdb_ms, polars_ms, datafusion_ms) =
        measure_incumbents(definition, path, threads, iterations, answer_value(&result));
    let (scale_factor, rows, bytes) = dataset_shape(path);

    Ok(SizePoint {
        scale_factor,
        rows,
        bytes,
        engine_ms,
        engine_scan_ms,
        engine_operators_ms,
        duckdb_ms,
        polars_ms,
        datafusion_ms,
    })
}

/// Measures the three incumbents at one dataset size.
///
/// Every incumbent runs at the same thread count as the engine, on the same
/// file, cold. A system that could not be measured is reported as `None`: a
/// missing figure must not read as a fast one, and a curve with a hole in it
/// is honest where a curve that aborted is not.
fn measure_incumbents(
    definition: &QueryDefinition,
    path: &Path,
    threads: usize,
    iterations: usize,
    ours: Option<f64>,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut measured: Vec<(Comparator, f64)> = Vec::new();

    for comparator in [Comparator::DuckDb, Comparator::Polars, Comparator::DataFusion] {
        // The SQL addresses the table the way that system addresses it: the
        // cold `DuckDB` reads the Parquet file straight from the path, and
        // the other two read it under a name.
        let query = ExternalQuery::new(
            definition.query.name(),
            sql_for(
                definition,
                comparator,
                Mode::Cold,
                path,
                &columnar_testdata::orders_path_for(path),
            ),
        );
        match external::run(comparator, Mode::Cold, &query, path, threads, iterations) {
            Ok(outcome) => {
                cross_check(comparator, path, ours, outcome.result_value);
                measured.push((comparator, outcome.query_ms));
            }
            Err(error) => {
                eprintln!(
                    "skipping {name} at {path}: {error}",
                    name = comparator.name(),
                    path = path.display()
                );
            }
        }
    }

    (
        figure_of(&measured, Comparator::DuckDb),
        figure_of(&measured, Comparator::Polars),
        figure_of(&measured, Comparator::DataFusion),
    )
}

/// The figure one incumbent produced, when it produced one.
fn figure_of(measured: &[(Comparator, f64)], comparator: Comparator) -> Option<f64> {
    measured.iter().find(|(name, _)| *name == comparator).map(|(_, ms)| *ms)
}

/// Checks that an incumbent computed the same answer as the engine.
///
/// A disagreement is a warning and not an abort: the two systems are still
/// two systems, and a reader is better served by a curve with a warning on
/// it than by no curve at all.
fn cross_check(comparator: Comparator, path: &Path, ours: Option<f64>, theirs: Option<f64>) {
    let (Some(ours), Some(theirs)) = (ours, theirs) else {
        return;
    };
    let tolerance = ANSWER_TOLERANCE * ours.abs().max(1.0);
    if (ours - theirs).abs() > tolerance {
        eprintln!(
            "WARNING: {name} computed {theirs} on {path}, the engine computed {ours}",
            name = comparator.name(),
            path = path.display()
        );
    }
}

/// The engine's answer as a number, when it has one.
fn answer_value(result: &columnar_engine::AggregateResult) -> Option<f64> {
    result.groups.first().and_then(|group| group.values.first()).and_then(|value| match value {
        columnar_engine::AggregateValue::Int(v) => Some(*v as f64),
        columnar_engine::AggregateValue::Float(v) => Some(*v),
        columnar_engine::AggregateValue::Null => None,
    })
}

/// The dataset's shape: its scale factor, its rows and its bytes.
///
/// The manifest is the record of what was written, and it is where the scale
/// factor comes from. A dataset without one -- a file produced by something
/// other than this repository's generator -- has no scale factor, and that is
/// reported as unknown rather than as zero: a report that says "scale factor
/// 0" for a dataset it knows nothing about is not a report. The rows and the
/// bytes then come from the file itself: the footer the engine reads, and the
/// file's own metadata.
fn dataset_shape(path: &Path) -> (Option<u32>, u64, u64) {
    let bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
    if let Ok(manifest) = columnar_testdata::DatasetManifest::read_json(path) {
        return (Some(manifest.scale_factor), manifest.rows, manifest.bytes);
    }
    let rows = columnar_engine::ParquetSource::open(path).map_or(0, |source| source.rows() as u64);
    (None, rows, bytes)
}

/// A figure, or a dash when there is none.
fn figure(ms: Option<f64>) -> String {
    match ms {
        Some(ms) if ms.is_finite() => format!("{ms:.1} ms"),
        _ => "-".to_owned(),
    }
}
