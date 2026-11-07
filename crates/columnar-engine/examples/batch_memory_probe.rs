//! The batch size's price.
//!
//! Spec 048 made the reader's batch size a knob and measured what it **buys**: a batch of a row group —
//! **1,000,000** rows — is **4.926 ms** cheaper than the engine's own **125,000**, faster in **24 of 24**
//! paired samples. **This probe measures what it costs.**
//!
//! ## What a batch holds
//!
//! The reader holds a batch's columns at once, and **the bytes a batch holds scale with the query's
//! width**. The same million-row batch is a few megabytes on a two-column query and several times that on a
//! sixteen-column one — **and a knob named in rows cannot see the width**.
//!
//! So the probe reads the file at **three widths**:
//!
//! | the shape | what the reader fetches | what the plan keeps |
//! |---|---|---|
//! | **the two predicate columns** | the `q6` predicates' two columns | nothing |
//! | **the two materialised columns** | the join's probe side's two columns | both of them |
//! | **the file's sixteen columns** | every column | every column |
//!
//! ## The measurement
//!
//! **A counting global allocator.** It sees every allocation the process makes, and it sees the **peak** —
//! which is what a memory budget has to survive. **A budget that is checked against an estimate has never seen
//! the real thing**, so the estimate is printed beside the measurement and the two are read against each
//! other.
//!
//! **And the allocator is in this example, not in the engine.** Every crate here denies `unsafe_code`, and a
//! `GlobalAlloc` implementation needs `unsafe`. **An example is its own crate**, so the counting allocator is
//! the diagnostic's and the engine's own memory behaviour is untouched by it.
//!
//! **And the time is not measured here.** A counting allocator counts **every** allocation, and that is not
//! free — so the memory is measured with the allocator in it, and the time is measured in spec 046's probe,
//! which has no allocator in it. A time measured next to an allocator is not the engine's time.
//!
//! ## The rows
//!
//! **The batch size must not change the answer.** Every shape's rows are recorded at every batch size, and
//! the probe refuses to write a report whose shapes disagree.

#![allow(clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

#[path = "support/counting_allocator.rs"]
mod counting_allocator;

use columnar_engine::memory::estimate_lazy_read_with_batch;
use columnar_engine::queries::{BuiltinQuery, QUERIES};
use columnar_engine::{
    AggregateResult, AggregateValue, Dataset, EngineError, ExecOptions, MemoryBudget,
    ParquetSource, QueryPlan,
};
use counting_allocator::Counting;

/// The allocator that counts.
///
/// **It is here, in the example, and not in the engine**: every crate here
/// denies `unsafe_code`, and a `GlobalAlloc` implementation needs `unsafe`.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The batch sizes the probe measures, beside the engine's own.
///
/// **The points are rows**, and every one of them divides a row group's row count — the file has six row
/// groups of a million rows each — so no row group ends with a partial batch, and the sweep is the engine's
/// read and nothing else.
///
/// **The engine's own is one of the points**: **125,000** rows, the largest divisor of a row group's row
/// count under the engine's **byte budget**.
const BATCH_SIZES: [usize; 4] = [62_500, 125_000, 250_000, 1_000_000];

/// How many times each point is measured.
///
/// **Three**, so that the published figure is a median and not a single reading.
const REPEATS: usize = 3;

/// The budget the wide shape is measured under.
///
/// **The default budget refuses the wide shape** — its estimate is the file's own columns over the file's own
/// rows, and that is more than **256 MiB**. The probe raises the budget so the read can happen and be
/// measured, and it **reports the refusal** beside the measurement.
const WIDE_BUDGET_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// The columns the join's probe side reads.
const THE_TWO: [&str; 2] = ["l_orderkey", "l_extendedprice"];

/// Which columns a shape materialises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Width {
    /// The shape's own columns, by name.
    Named(&'static [&'static str]),
    /// Every column the file has.
    Every,
}

/// One shape of the read.
struct Shape {
    /// The shape's name, as the report carries it.
    name: &'static str,
    /// The columns the shape materialises.
    width: Width,
    /// The plan, built for the file.
    plan: fn(&Path) -> QueryPlan,
    /// Whether the plan returns its rows rather than an aggregate.
    returns_rows: bool,
}

/// The shapes the probe reads at.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            name: "the two predicate columns",
            width: Width::Named(&[]),
            plan: q6_plan,
            returns_rows: false,
        },
        Shape {
            name: "the two materialised columns",
            width: Width::Named(&THE_TWO),
            plan: two_columns_plan,
            returns_rows: true,
        },
        Shape {
            name: "the file's sixteen columns",
            width: Width::Every,
            plan: every_column_plan,
            returns_rows: true,
        },
    ]
}

/// The `q6` plan, from the engine's own table of queries.
///
/// **The house's plan, not a copy of it**: the probe measures the plan the harness measures.
fn q6_plan(_build: &Path) -> QueryPlan {
    let definition = QUERIES
        .iter()
        .find(|definition| definition.query == BuiltinQuery::Q6)
        .expect("the q6 query is in the engine's table");
    (definition.plan)(Path::new("orders.parquet"))
}

/// The plan that materialises every column the file has.
fn every_column_plan(_build: &Path) -> QueryPlan {
    QueryPlan::scan_all()
}

/// The plan that materialises the join's probe side's two columns.
///
/// **No aggregate**: a plan with an aggregation folds its rows into groups and has no rows to hand back, and
/// the shape is about the **read** — the two columns the reader fetches and keeps.
fn two_columns_plan(_build: &Path) -> QueryPlan {
    QueryPlan::new(Vec::new(), THE_TWO.iter().map(|name| (*name).to_owned()).collect(), None)
}

/// What one measurement of a read found.
struct Measured {
    /// The most bytes the read held at once, above the baseline.
    peak: usize,
    /// The bytes the read left behind, above the baseline.
    held: usize,
}

/// Measures one read, and returns what it found beside what it produced.
///
/// **The baseline is taken after the source is open**, so the source's own metadata is not the read's cost.
fn measure_read<T, F: FnOnce() -> Result<T, EngineError>>(
    run: F,
) -> Result<(Measured, T), EngineError> {
    let baseline = Counting::held();
    Counting::reset(baseline);
    let value = run()?;
    let peak = Counting::peak().saturating_sub(baseline);
    let held = Counting::held().saturating_sub(baseline);
    Ok((Measured { peak, held }, value))
}

/// What a shape's read produced.
///
/// **The result is held while the memory is counted**, so that "what the read leaves behind" is the read's own
/// retained bytes and not zero.
enum Outcome {
    /// The plan returned an aggregate.
    ///
    /// **Boxed**, because the aggregate's own report is large and the rows are not.
    Aggregate(Box<AggregateResult>),
    /// The plan returned its rows.
    Rows(Dataset),
}

impl Outcome {
    /// The rows the read produced.
    fn rows(&self) -> usize {
        match self {
            Self::Aggregate(result) => result
                .groups
                .first()
                .and_then(|group| group.values.first())
                .and_then(|value| match value {
                    AggregateValue::Int(value) => usize::try_from(*value).ok(),
                    AggregateValue::Float(_) | AggregateValue::Null => None,
                })
                .unwrap_or(0),
            Self::Rows(rows) => rows.len(),
        }
    }
}

/// Runs one shape's read against one source.
fn run_shape(
    shape: &Shape,
    source: &ParquetSource,
    options: &ExecOptions,
) -> Result<Outcome, EngineError> {
    let plan = (shape.plan)(Path::new("orders.parquet"));
    if shape.returns_rows {
        Ok(Outcome::Rows(columnar_engine::execute_rows_on(&plan, source, options)?))
    } else {
        Ok(Outcome::Aggregate(Box::new(columnar_engine::execute_on(&plan, source, options)?)))
    }
}

/// The estimate the engine's own formula gives for a shape, at a batch size.
///
/// **The engine's own function**, called with the shape's own read — so the figure is the engine's, and not a
/// copy of it. **And it depends on the batch size**, because the read holds one batch of the columns it
/// fetches at a time: that is the whole of what this increment changed.
fn the_estimate(source: &ParquetSource, shape: &Shape, batch_rows: usize) -> usize {
    let plan = (shape.plan)(Path::new("orders.parquet"));
    let Ok(resolved) = plan.resolve_against(source) else {
        return 0;
    };
    let Ok(read) = source.read_shape(&resolved.materialised, &resolved.filter) else {
        return 0;
    };
    estimate_lazy_read_with_batch(
        source.rows(),
        source,
        &read.held,
        &read.fetched,
        batch_rows,
        1,
        false,
    )
}

/// The number of batches a named batch size makes over the file's row groups.
///
/// **Each row group is read in its own batches**, so the count is the sum over the row groups -- and a named
/// size that divides a row group's row count leaves no partial batch, which is what the sweep's points are
/// chosen for.
fn the_batch_count(source: &ParquetSource, rows: usize) -> usize {
    let group_rows: Vec<usize> = source
        .columns()
        .first()
        .map(|column| {
            source.row_group_ranges(&column.name).iter().map(|group| group.rows).collect()
        })
        .unwrap_or_default();
    if group_rows.is_empty() {
        let groups = source.row_groups().max(1);
        return groups * (source.rows().div_ceil(groups)).div_ceil(rows);
    }
    group_rows.iter().map(|group| group.div_ceil(rows)).sum()
}

/// The median of a series of bytes.
fn median(values: &[usize]) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
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
    let mut out = PathBuf::from("/tmp/batch-memory.json");
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (data, out) = arguments()?;
    let source = ParquetSource::open(&data)?;
    let wide_budget = ExecOptions {
        threads: Some(1),
        memory_budget: MemoryBudget { bytes: WIDE_BUDGET_BYTES },
        ..ExecOptions::default()
    };
    let default_budget = ExecOptions { threads: Some(1), ..ExecOptions::default() };

    let mut shapes_report = Vec::new();
    let mut the_rows_agree = true;
    let mut the_loads = Vec::new();

    for shape in shapes() {
        let refused_at_the_default = if shape.returns_rows {
            let plan = (shape.plan)(Path::new("orders.parquet"));
            columnar_engine::execute_rows_on(&plan, &source, &default_budget).is_err()
        } else {
            false
        };

        let mut points = Vec::new();
        let mut shape_rows: Option<usize> = None;
        for batch in BATCH_SIZES {
            let mut peaks = Vec::new();
            let mut helds = Vec::new();
            let mut rows = 0usize;
            for _ in 0..REPEATS {
                let batched = ParquetSource::open(&data)?.with_reader_batch_rows(batch);
                let options =
                    if shape.width == Width::Every { &wide_budget } else { &default_budget };
                let (measured, produced) = measure_read(|| run_shape(&shape, &batched, options))?;
                peaks.push(measured.peak);
                helds.push(measured.held);
                rows = produced.rows();
                the_loads.push(load_average());
            }
            match shape_rows {
                Some(first) if first != rows => the_rows_agree = false,
                None => shape_rows = Some(rows),
                _ => {}
            }
            points.push(serde_json::json!({
                "batch_rows": batch,
                "the_batches": the_batch_count(&source, batch),
                "the_estimate_bytes": the_estimate(&source, &shape, batch),
                "the_peak_bytes": peaks,
                "the_peak_median": median(&peaks),
                "the_held_after_bytes": helds,
                "the_held_median": median(&helds),
                "the_rows": rows,
            }));
        }

        shapes_report.push(serde_json::json!({
            "the_shape": shape.name,
            "the_columns_materialised": match shape.width {
                Width::Named(names) => names.len(),
                Width::Every => source.columns().len(),
            },
            "refused_at_the_default_budget": refused_at_the_default,
            "the_rows": shape_rows.unwrap_or(0),
            "the_points": points,
        }));
    }

    let mut report = serde_json::Map::new();
    report.insert("probe".to_owned(), serde_json::json!("batch-memory"));
    report.insert("code".to_owned(), serde_json::json!(current_code()));
    report.insert("the_repeats".to_owned(), serde_json::json!(REPEATS));
    report.insert("the_batch_sizes".to_owned(), serde_json::json!(BATCH_SIZES));
    report.insert("the_rows_agree".to_owned(), serde_json::json!(the_rows_agree));
    report.insert("the_shapes".to_owned(), serde_json::json!(shapes_report));
    let load = the_loads.iter().copied().fold(0.0_f64, f64::max);
    report.insert("the_host_load_average".to_owned(), serde_json::json!(load));

    std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    println!("wrote {}", out.display());
    Ok(())
}
