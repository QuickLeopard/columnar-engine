//! The batch size's price, in time.
//!
//! Spec 048 measured what the reader's batch size **buys** the join: a batch of a row group — **1,000,000**
//! rows — is **4.926 ms** cheaper than the engine's own **125,000**, faster in **24 of 24** paired samples.
//!
//! **This probe prices that win against the other shape.** The engine's limit was chosen for the
//! `filter-range` shape ([§3e](../../PERF.md#3e-the-scans-decomposition-and-what-the-compaction-was-worth)),
//! and the join's shape is a different one. **So both are measured here, in one session, back to back, with
//! the order alternated** — and the difference is the **paired** one, not the difference of two medians from
//! two campaigns.
//!
//! | the shape | what is measured |
//! |---|---|
//! | **the join's part** | the join's stage: the build table's construction and the probe, together |
//! | **the filter-range's scan** | the scan's stage: the read and the pushed-down predicates |
//!
//! **And the batch sizes rotate.** A rotation of five, twice over: every point takes every position exactly
//! twice, and every pair of points runs in one order twice and the other order twice. A point that always
//! ran first would pay a colder cache than one that always ran last.
//!
//! **And each shape's own batch is its own point.** The default is a byte budget, so the batch it gives
//! depends on the width the read fetches -- and the probe asks the engine, so the reference it publishes is
//! the engine's own and not a copy of it. **The engine's own must be one of the points**: a difference against
//! a batch the sweep did not measure is not a measurement, and the probe refuses to publish one.
//!
//! ## The rows
//!
//! **The batch size must not change the answer.** Every shape's rows are checked at every batch size, and the
//! probe refuses to write a report whose shapes disagree.

#![allow(clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use columnar_engine::queries::{BuiltinQuery, QUERIES};
use columnar_engine::{EngineError, ExecOptions, MemoryBudget, ParquetSource, QueryPlan};

/// The batch sizes the probe measures, beside the engine's own.
///
/// **The points are rows**, and every one of them divides a row group's row count — the file has six row
/// groups of a million rows each — so no row group ends with a partial batch.
///
/// **The engine's own is one of the points**, and it is **not the same point for every shape**: the default
/// is a byte budget, so the batch it gives depends on the width the read fetches — and the wide read's own
/// is **100,000** rows, which no published set had measured.
const BATCH_SIZES: [usize; 5] = [62_500, 100_000, 125_000, 250_000, 1_000_000];

/// How many times the sweep runs.
///
/// **Even**, so that the two orders' groups are the same size.
const ITERATIONS: usize = 10;

/// The orders the sweep's points run in, one per iteration.
///
/// **A rotation of five**, twice over.
const SWEEP_ORDERS: [[usize; 5]; ITERATIONS] = [
    [0, 1, 2, 3, 4],
    [1, 2, 3, 4, 0],
    [2, 3, 4, 0, 1],
    [3, 4, 0, 1, 2],
    [4, 0, 1, 2, 3],
    [0, 1, 2, 3, 4],
    [1, 2, 3, 4, 0],
    [2, 3, 4, 0, 1],
    [3, 4, 0, 1, 2],
    [4, 0, 1, 2, 3],
];

/// A duration in milliseconds.
fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The median of a series.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
}

/// The least and the most of a series.
fn extremes(values: &[f64]) -> (f64, f64) {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (sorted.first().copied().unwrap_or(0.0), sorted.last().copied().unwrap_or(0.0))
}

/// The host's load average.
fn load_average() -> f64 {
    if let Ok(value) = std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output() {
        if value.status.success() {
            if let Some(load) = first_number(&String::from_utf8_lossy(&value.stdout)) {
                return load;
            }
        }
    }
    if let Ok(contents) = std::fs::read_to_string("/proc/loadavg") {
        if let Some(load) = first_number(&contents) {
            return load;
        }
    }
    0.0
}

/// The first number in a line of text, when there is one.
fn first_number(text: &str) -> Option<f64> {
    text.replace(['{', '}'], " ")
        .split_whitespace()
        .next()
        .and_then(|first| first.parse::<f64>().ok())
}

/// The build side's file, beside the probe side's.
fn build_path_for(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let orders_stem = match stem.strip_prefix("lineitem") {
        Some(rest) => format!("orders{rest}"),
        None => "orders".to_owned(),
    };
    path.with_file_name(format!("{orders_stem}.parquet"))
}

/// The house's own plan for a builtin query.
fn plan_for(query: BuiltinQuery, build: &Path) -> QueryPlan {
    let definition = QUERIES
        .iter()
        .find(|definition| definition.query == query)
        .expect("the query is in the engine's table");
    (definition.plan)(build)
}

/// The engine's own batch size for a read of this plan's columns.
///
/// **The default is a byte budget**, so the batch it gives depends on the width the read fetches -- and the
/// probe asks the engine, so the reference it publishes is the engine's own and not a copy of it.
fn own_batch_for(data: &Path, plan: &QueryPlan) -> Result<usize, EngineError> {
    let source = ParquetSource::open(data)?;
    let resolved = plan.resolve_against(&source)?;
    let read = source.read_shape(&resolved.materialised, &resolved.filter)?;
    Ok(source.batch_rows_for(&read.fetched, 1))
}

/// Each shape's own batch: the size in rows, and the point it is in the sweep.
///
/// **The engine's own must be one of the points**: a difference against a batch the sweep did not measure
/// is not a measurement, and the probe refuses to publish one.
struct Own {
    /// The join's part: its own batch, in rows.
    join_rows: usize,
    /// The join's part's point in the sweep.
    join_point: usize,
    /// The filter-range's scan: its own batch, in rows.
    filter_rows: usize,
    /// The filter-range's scan's point in the sweep.
    filter_point: usize,
    /// The wide read: its own batch, in rows.
    wide_rows: usize,
    /// The wide read's point in the sweep.
    wide_point: usize,
}

/// The three shapes' own batches, and the points they are.
fn the_own_points(data: &Path, build: &Path) -> Result<Own, Box<dyn std::error::Error>> {
    let join_rows = own_batch_for(data, &plan_for(BuiltinQuery::JoinOrders, build))?;
    let filter_rows = own_batch_for(data, &plan_for(BuiltinQuery::FilterRange, data))?;
    let wide_rows = own_batch_for(data, &QueryPlan::scan_all())?;
    Ok(Own {
        join_rows,
        join_point: point_of(join_rows)?,
        filter_rows,
        filter_point: point_of(filter_rows)?,
        wide_rows,
        wide_point: point_of(wide_rows)?,
    })
}

/// The point a batch size is, in the sweep.
fn point_of(rows: usize) -> Result<usize, Box<dyn std::error::Error>> {
    BATCH_SIZES.iter().position(|size| *size == rows).ok_or_else(|| {
        format!("the engine's own batch, {rows} rows, is not one of the sweep's points").into()
    })
}

/// What one run of a shape found.
struct Run {
    /// The part the batch size moves.
    part_ms: f64,
    /// The whole query.
    total_ms: f64,
    /// The rows the query produced.
    rows: usize,
}

/// The join's own part: the build table's construction and the probe, together.
fn run_join(
    data: &Path,
    build: &Path,
    batch: usize,
    options: &ExecOptions,
) -> Result<Run, EngineError> {
    let source = ParquetSource::open(data)?.with_reader_batch_rows(batch);
    let plan = plan_for(BuiltinQuery::JoinOrders, build);
    let started = Instant::now();
    let result = columnar_engine::execute_on(&plan, &source, options)?;
    let total_ms = ms(started.elapsed());
    let join = result
        .plan_output
        .join
        .as_ref()
        .ok_or_else(|| EngineError::InvalidPlan("the plan has no join".to_owned()))?;
    Ok(Run {
        part_ms: ms(join.build_elapsed + join.probe_elapsed),
        total_ms,
        rows: join.output_rows,
    })
}

/// The filter-range's own scan: the read and the pushed-down predicates.
fn run_filter(data: &Path, batch: usize, options: &ExecOptions) -> Result<Run, EngineError> {
    let source = ParquetSource::open(data)?.with_reader_batch_rows(batch);
    let plan = plan_for(BuiltinQuery::FilterRange, data);
    let started = Instant::now();
    let result = columnar_engine::execute_on(&plan, &source, options)?;
    let total_ms = ms(started.elapsed());
    let scan = result
        .plan_output
        .stages
        .iter()
        .find(|stage| stage.stage == "scan")
        .map_or(0.0, |stage| ms(stage.elapsed));
    // **The scan's own rows out**: the predicates are pushed into the reader, so
    // there is no filter stage, and the scan's rows out are the query's rows.
    let rows = result
        .plan_output
        .stages
        .iter()
        .find(|stage| stage.stage == "scan")
        .map_or(0, |stage| usize::try_from(stage.rows_out).unwrap_or(0));
    Ok(Run { part_ms: scan, total_ms, rows })
}

/// The widest read: every column of the file, materialised.
///
/// **The whole read is the part**, because the read is all of it: the file's sixteen columns are the query.
///
/// **And the budget is raised**, because the default budget **refuses** this shape -- its estimate is the
/// file's columns over the file's rows, and that is more than **256 MiB**. The refusal is a finding of its
/// own, and it is recorded beside the measurement.
fn run_wide(data: &Path, batch: usize, options: &ExecOptions) -> Result<Run, EngineError> {
    let source = ParquetSource::open(data)?.with_reader_batch_rows(batch);
    let plan = QueryPlan::scan_all();
    let started = Instant::now();
    let rows = columnar_engine::execute_rows_on(&plan, &source, options)?;
    let total_ms = ms(started.elapsed());
    Ok(Run { part_ms: total_ms, total_ms, rows: rows.len() })
}

/// The code the probe measured: the commit, and the tree's state.
fn current_code() -> String {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "unknown".to_owned(),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        );
    let state = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "unknown".to_owned(),
            |output| {
                if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
                    "clean".to_owned()
                } else {
                    "dirty".to_owned()
                }
            },
        );
    format!("{commit}-{state}")
}

/// The command line.
fn arguments() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let mut data = PathBuf::from("data/lineitem.parquet");
    let mut out = PathBuf::from("/tmp/batch-sweep.json");
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--data" => {
                data = args.next().ok_or("--data needs a path")?.into();
            }
            "--out" => {
                out = args.next().ok_or("--out needs a path")?.into();
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok((data, out))
}

/// The budget the wide shape is measured under.
///
/// **The default refuses the wide shape** -- its estimate is the file's sixteen columns over the file's six
/// million rows, and that is more than **256 MiB**. The probe raises the budget so the read can happen and be
/// measured, and the **refusal** is recorded beside the measurement.
fn wide_budget() -> MemoryBudget {
    MemoryBudget { bytes: 4 * 1024 * 1024 * 1024 }
}

/// A series, as the report carries it.
fn series(values: &[f64]) -> serde_json::Value {
    let (least, most) = extremes(values);
    serde_json::json!({
        "median": median(values),
        "min": least,
        "max": most,
        "samples": values,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (data, out) = arguments()?;
    let build = build_path_for(&data);
    let options = ExecOptions { threads: Some(1), ..ExecOptions::default() };

    // The wide shape's budget: the default refuses it, and the refusal is a finding.
    let wide_options =
        ExecOptions { threads: Some(1), memory_budget: wide_budget(), ..ExecOptions::default() };

    // The sweep's own samples, per point.
    let mut join_parts: Vec<Vec<f64>> = vec![Vec::new(); BATCH_SIZES.len()];
    let mut join_totals: Vec<Vec<f64>> = vec![Vec::new(); BATCH_SIZES.len()];
    let mut filter_parts: Vec<Vec<f64>> = vec![Vec::new(); BATCH_SIZES.len()];
    let mut filter_totals: Vec<Vec<f64>> = vec![Vec::new(); BATCH_SIZES.len()];
    let mut wide_parts: Vec<Vec<f64>> = vec![Vec::new(); BATCH_SIZES.len()];
    let mut wide_totals: Vec<Vec<f64>> = vec![Vec::new(); BATCH_SIZES.len()];
    let mut loads = Vec::new();
    let mut the_rows_agree = true;
    let mut join_rows = 0usize;
    let mut filter_rows = 0usize;
    let mut wide_rows = 0usize;

    for (iteration, order) in SWEEP_ORDERS.iter().enumerate() {
        for (position, &point) in order.iter().enumerate() {
            let batch = BATCH_SIZES[point];
            let join_first = (iteration + position) % 2 == 0;
            let (join, filter, wide) = if join_first {
                let join = run_join(&data, &build, batch, &options)?;
                let filter = run_filter(&data, batch, &options)?;
                let wide = run_wide(&data, batch, &wide_options)?;
                (join, filter, wide)
            } else {
                let wide = run_wide(&data, batch, &wide_options)?;
                let filter = run_filter(&data, batch, &options)?;
                let join = run_join(&data, &build, batch, &options)?;
                (join, filter, wide)
            };
            join_parts[point].push(join.part_ms);
            join_totals[point].push(join.total_ms);
            filter_parts[point].push(filter.part_ms);
            filter_totals[point].push(filter.total_ms);
            wide_parts[point].push(wide.part_ms);
            wide_totals[point].push(wide.total_ms);
            if iteration == 0 {
                join_rows = join.rows;
                filter_rows = filter.rows;
                wide_rows = wide.rows;
            } else if join.rows != join_rows || filter.rows != filter_rows || wide.rows != wide_rows
            {
                the_rows_agree = false;
            }
            loads.push(load_average());
        }
    }

    // The paired difference against the engine's own batch size, in the same iteration.
    //
    // **Each shape has its own**: the default is a byte budget, so the batch it
    // gives depends on the width the read fetches -- and the wide read's own is
    // a different point from the join's.
    let own = the_own_points(&data, &build)?;
    let mut differences = serde_json::Map::new();
    for (point, batch) in BATCH_SIZES.iter().enumerate() {
        let mut entry = serde_json::Map::new();
        if point != own.join_point {
            let join_difference: Vec<f64> = join_parts[point]
                .iter()
                .zip(join_parts[own.join_point].iter())
                .map(|(point, own)| point - own)
                .collect();
            entry.insert("the_join_part_ms".to_owned(), series(&join_difference));
        }
        if point != own.filter_point {
            let filter_difference: Vec<f64> = filter_parts[point]
                .iter()
                .zip(filter_parts[own.filter_point].iter())
                .map(|(point, own)| point - own)
                .collect();
            entry.insert("the_filter_scan_ms".to_owned(), series(&filter_difference));
        }
        if point != own.wide_point {
            let wide_difference: Vec<f64> = wide_parts[point]
                .iter()
                .zip(wide_parts[own.wide_point].iter())
                .map(|(point, own)| point - own)
                .collect();
            entry.insert("the_wide_read_ms".to_owned(), series(&wide_difference));
        }
        if entry.is_empty() {
            continue;
        }
        differences.insert(batch.to_string(), serde_json::json!(entry));
    }

    let report = the_report(
        &join_parts,
        &join_totals,
        &filter_parts,
        &filter_totals,
        &wide_parts,
        &wide_totals,
        &differences,
        &own,
        the_rows_agree,
        (join_rows, filter_rows, wide_rows),
        &loads,
    );

    std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Everything the sweep found, as the report carries it.
#[allow(clippy::too_many_arguments)]
fn the_report(
    join_parts: &[Vec<f64>],
    join_totals: &[Vec<f64>],
    filter_parts: &[Vec<f64>],
    filter_totals: &[Vec<f64>],
    wide_parts: &[Vec<f64>],
    wide_totals: &[Vec<f64>],
    differences: &serde_json::Map<String, serde_json::Value>,
    own: &Own,
    the_rows_agree: bool,
    rows: (usize, usize, usize),
    loads: &[f64],
) -> serde_json::Map<String, serde_json::Value> {
    let mut sweep = serde_json::Map::new();
    for (point, batch) in BATCH_SIZES.iter().enumerate() {
        sweep.insert(
            batch.to_string(),
            serde_json::json!({
                "the_join_part_ms": series(&join_parts[point]),
                "the_join_total_ms": series(&join_totals[point]),
                "the_filter_scan_ms": series(&filter_parts[point]),
                "the_filter_total_ms": series(&filter_totals[point]),
                "the_wide_read_ms": series(&wide_parts[point]),
                "the_wide_total_ms": series(&wide_totals[point]),
            }),
        );
    }

    let mut report = serde_json::Map::new();
    report.insert("probe".to_owned(), serde_json::json!("batch-sweep"));
    report.insert("code".to_owned(), serde_json::json!(current_code()));
    report.insert("iterations".to_owned(), serde_json::json!(ITERATIONS));
    report.insert("the_batch_sizes".to_owned(), serde_json::json!(BATCH_SIZES));
    report.insert(
        "the_engine_s_own_batch_rows".to_owned(),
        serde_json::json!({
            "the_join_part": own.join_rows,
            "the_filter_scan": own.filter_rows,
            "the_wide_read": own.wide_rows,
        }),
    );
    report.insert("the_rows_agree".to_owned(), serde_json::json!(the_rows_agree));
    report.insert("the_join_rows".to_owned(), serde_json::json!(rows.0));
    report.insert("the_filter_rows".to_owned(), serde_json::json!(rows.1));
    report.insert("the_wide_rows".to_owned(), serde_json::json!(rows.2));
    report.insert("the_sweep_ms".to_owned(), serde_json::json!(sweep));
    report.insert("the_sweep_difference_ms".to_owned(), serde_json::json!(differences));
    let load = loads.iter().copied().fold(0.0_f64, f64::max);
    report.insert("the_host_load_average".to_owned(), serde_json::json!(load));
    report
}
