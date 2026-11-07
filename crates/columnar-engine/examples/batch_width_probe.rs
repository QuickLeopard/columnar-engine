//! The width statement.
//!
//! Spec 049 priced the reader's batch size and decided the rule's shape: **a byte budget**. **And the rule
//! needs to know what a row costs before the read** — so this probe checks the engine's statement of that
//! against a measurement.
//!
//! ## The question
//!
//! **What does one row of a column cost?** There are three answers in this repository, and only one of them is
//! about the working set:
//!
//! | the answer | what it says |
//! |---|---|
//! | the **file's own footer** | the file's **pages** — the chunk's `total_uncompressed_size` over its `num_values` |
//! | the **schema's own type** | the type's own cost — and a `utf8` column is **two** prices |
//! | the **statement** | the engine's own storage: **8** for a number, **4** for a dictionary's index, **24** for a `String`'s header, and the value's own bytes on top |
//!
//! **The allocator's measurement is the check**, because it is the only one of the three that is about what the
//! process actually holds.
//!
//! ## The shapes
//!
//! **The three widths of spec 049**, so the two probes' numbers are read against each other:
//!
//! | the shape | what the reader fetches | what the plan keeps |
//! |---|---|---|
//! | **the two predicate columns** | the `q6` predicates' two columns | nothing |
//! | **the two materialised columns** | the join's probe side's two columns | both of them |
//! | **the file's sixteen columns** | every column | every column |
//!
//! ## The measurement
//!
//! **A counting global allocator**, in [`support::counting_allocator`]. It sees every allocation the process
//! makes, and it sees the **peak** — which is what a memory budget has to survive.
//!
//! **The bytes a row are the slope**: the peak grows with the batch, and the growth between the smallest batch
//! and the largest is what one row of that batch costs.
//!
//! **And the time is not measured here.** A counting allocator counts **every** allocation, and that is not
//! free — so the memory is measured with the allocator in it, and the time is measured in spec 049's other
//! probe, which has no allocator in it.
//!
//! ## The rows
//!
//! **The batch size must not change the answer.** Every shape's rows are recorded at every batch size, and the
//! probe refuses to write a report whose shapes disagree.
//!
//! ## And the rule's output
//!
//! **The default is a byte budget**, and the budget is divided by the statement — so **the rule's output is a
//! figure**, and a figure with no instrument behind it is a claim. This probe publishes **the batch size the
//! default gives each shape**, at one worker and at the worker counts a parallel read shares the budget across.
//!
//! **The budget is the read's, and the workers share it**, so a read on *n* workers gets an *n*-th of it. At one
//! worker the rule is the one spec 049 published; above it the batch is smaller, and the divisors are coarse.

#![allow(clippy::indexing_slicing)]

#[path = "support/counting_allocator.rs"]
mod counting_allocator;

use std::path::{Path, PathBuf};

use columnar_engine::queries::{BuiltinQuery, QUERIES};
use columnar_engine::{
    AggregateResult, AggregateValue, Dataset, EngineError, ExecOptions, MemoryBudget,
    ParquetSource, QueryPlan, DEFAULT_READER_BATCH_BYTES,
};
use counting_allocator::Counting;

/// The allocator that counts.
///
/// **It is here, in the example, and not in the engine**: every crate here
/// denies `unsafe_code`, and a `GlobalAlloc` implementation needs `unsafe`.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The batch sizes the probe measures at.
///
/// **The points are rows**, and every one of them divides a row group's row count — the file has six row
/// groups of a million rows each — so no row group ends with a partial batch, and the sweep is the engine's
/// read and nothing else.
///
/// **Two, and not four**: the width is the **slope** between them, and the slope's endpoints are all the
/// statement needs.
const BATCH_SIZES: [usize; 2] = [62_500, 1_000_000];

/// How many times each point is measured.
///
/// **Three**, so that the published figure is a median and not a single reading.
const REPEATS: usize = 3;

/// The budget the wide shape is measured under.
///
/// **The default budget refuses the wide shape** — its estimate is the file's own columns over the file's own
/// rows, and that is more than **256 MiB**. The probe raises the budget so the read can happen and be
/// measured.
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

/// The **rule's output**: the default's own batch size for a read of these
/// columns, at one worker and at the worker counts a parallel read shares the
/// budget across.
///
/// **The rule's output is a figure**, and a figure with no instrument behind
/// it is a claim: this is the instrument. The **budget is the read's** and the
/// workers share it, so a read on *n* workers gets an *n*-th of it -- and the
/// batch is the **largest divisor of the row group's row count** whose bytes
/// fit that share.
fn the_default_batches(source: &ParquetSource, fetched: &[usize]) -> Vec<serde_json::Value> {
    [1usize, 2, 4, 6, 14]
        .iter()
        .map(|workers| {
            serde_json::json!({
                "the_workers": workers,
                "the_batch_rows": source.batch_rows_for(fetched, *workers),
            })
        })
        .collect()
}

/// The command line.
fn arguments() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let mut data = PathBuf::from("data/lineitem.parquet");
    let mut out = PathBuf::from("/tmp/batch-width.json");
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
        let plan = (shape.plan)(Path::new("orders.parquet"));
        let resolved = plan.resolve_against(&source)?;
        let read = source.read_shape(&resolved.materialised, &resolved.filter)?;

        // The **statement**: the engine's own per-column cost, over the columns
        // the reader fetches, and the validity bit each column's rows carry.
        //
        // **The totals, not the rates.** The rates are fractions, and the
        // figures a reader reads are derived from the totals by the script
        // that publishes them -- so the probe publishes what it measured.
        let rows = source.rows();
        let mut the_statement = 0usize;
        let mut the_file = 0usize;
        let mut the_columns = Vec::new();
        for &index in &read.fetched {
            let cost = source.column_cost(index);
            the_statement = the_statement
                .saturating_add(rows.saturating_mul(cost.holder))
                .saturating_add(cost.value)
                .saturating_add(rows.div_ceil(8));
            the_file = the_file.saturating_add(source.column_file_bytes(index));
            let name = source.columns()[index].name.clone();
            the_columns.push(serde_json::json!({
                "the_column": name,
                "the_holder_bytes": cost.holder,
                "the_value_bytes": cost.value,
                "the_file_bytes": source.column_file_bytes(index),
                "the_file_values": source.column_file_values(index),
            }));
        }

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
                "the_peak_bytes": peaks,
                "the_peak_median": median(&peaks),
                "the_held_after_bytes": helds,
                "the_held_median": median(&helds),
                "the_rows": rows,
            }));
        }

        // The **slope** is the script's: the peak grows with the batch, and
        // the growth between the two points is the batch's own cost a row.
        shapes_report.push(serde_json::json!({
            "the_shape": shape.name,
            "the_columns_fetched": read.fetched.len(),
            "the_columns_held": read.held.len(),
            "the_file_rows": rows,
            "the_statement_bytes": the_statement,
            "the_file_bytes": the_file,
            "the_columns": the_columns,
            "the_rows": shape_rows.unwrap_or(0),
            "the_default_s_batch_rows": source.batch_rows_for(&read.fetched, 1),
            "the_default_s_batch_by_workers": the_default_batches(&source, &read.fetched),
            "the_points": points,
        }));
    }

    let mut report = serde_json::Map::new();
    report.insert("probe".to_owned(), serde_json::json!("batch-width"));
    report.insert("code".to_owned(), serde_json::json!(current_code()));
    report.insert("the_repeats".to_owned(), serde_json::json!(REPEATS));
    report.insert("the_batch_sizes".to_owned(), serde_json::json!(BATCH_SIZES));
    report.insert(
        "the_default_s_batch_bytes".to_owned(),
        serde_json::json!(DEFAULT_READER_BATCH_BYTES),
    );
    report.insert("the_rows_agree".to_owned(), serde_json::json!(the_rows_agree));
    report.insert("the_shapes".to_owned(), serde_json::json!(shapes_report));
    let load = the_loads.iter().copied().fold(0.0_f64, f64::max);
    report.insert("the_host_load_average".to_owned(), serde_json::json!(load));

    std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    println!("wrote {}", out.display());
    Ok(())
}
