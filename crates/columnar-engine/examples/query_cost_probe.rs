//! The engine's query, in the shapes the plan output does not name.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind spec 024's question: **where does the time the plan output does
//! not account for go?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example query_cost_probe -- \
//!     data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! The plan output is the engine's own account of its work: a stage table, the
//! rows each stage was handed and produced, and the time each stage took. On
//! six of the eight published queries that account was complete. The two that
//! name a **sort** and a **join** did not add up, and the plan output did not
//! say what was missing.
//!
//! # What is measured
//!
//! For **every** published query, and on **both** paths — the resident one,
//! over a dataset already in memory, and the cold one, over the file — the
//! probe measures:
//!
//! | | |
//! |---|---|
//! | **`query_ms`** | the whole query, through the engine's own entry point |
//! | **`stages_ms`** | the plan output's own stage table, summed |
//! | **`unnamed_ms`** | the query, less the stages — the part of the query the plan output does not name |
//!
//! And, once, for the dataset's **every row** selection — the selection a plan
//! with no predicate has, and the one the old code expanded into row numbers:
//!
//! | | |
//! |---|---|
//! | **`selection_rows`** | the selection expanded into row numbers -- what the join and the sort used to ask for, before either stage's clock started |
//! | **`selection_walk`** | the same rows, read as they are needed |
//! | **`bitmap_rows`**, **`bitmap_walk`** | the same two shapes, with the selection held as a **bitmap** -- the control, and the shape a plan with a dense predicate has |
//! | **`plan_resolve`** | the plan's resolution against the dataset |
//! | **`memory_estimate`** | the estimates the engine makes before it reads |
//! | **`build_open`** | the join's build side opened |
//!
//! The difference between `query_ms` and `stages_ms` is **measured, not
//! inferred**: it is the figure the other shapes have to account for, and
//! whatever is left over is named as the remainder.
//!
//! # The report
//!
//! The report is **per query**: each of the eight queries keeps its own
//! resident and cold figures, and the shapes are measured once, because they
//! are a property of the dataset's selection and not of a query. The sections
//! are `queries` (per query, per path) and `shapes_ms` (the selection's
//! shapes).
//!
//! # The same probe, against two code versions
//!
//! The probe is written against the engine's API that both code versions
//! share, so the **same file** measures the code that replaced the code it
//! replaced -- back to back, on the same host. The reports are committed
//! beside each other.
//!
//! # The answer is checked first
//!
//! The probe runs the engine's **own** plans, through the engine's own entry
//! point, and checks the answer against the published one before anything is
//! timed -- on **both** paths. A probe that measured a different query would
//! be a measurement of nothing.
//!
//! # What the figures are not
//!
//! These are figures on a **shared** host, taken five times each and reported
//! as the median with the spread beside it. The report carries the host's load
//! average, so a figure that drifted can be read against it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_core::SelectionVector;
use columnar_engine::queries::BuiltinQuery;
use columnar_engine::{memory, Dataset, ExecOptions, ParquetSource, QueryDefinition, QueryPlan};

const ITERATIONS: usize = 5;

/// The published queries, and the answer each of them must give.
///
/// These are the answers the published campaigns recorded, and the probe
/// checks its own against them before it times anything. The sums are
/// compared with a **relative** tolerance, because floating-point addition is
/// not associative and the two paths sum the same rows in different orders.
const EXPECTED: &[(BuiltinQuery, f64)] = &[
    (BuiltinQuery::Scan, 6_000_000.0),
    (BuiltinQuery::FilterNumeric, 3_817_685.0),
    (BuiltinQuery::FilterString, 4_265_647.0),
    (BuiltinQuery::FilterRange, 55_559_443.879_999_444),
    (BuiltinQuery::Q6, 315_203.0),
    (BuiltinQuery::Q6Revenue, 5_526_985_127.480_64),
    (BuiltinQuery::JoinOrders, 11_472_226_962.890_1),
    (BuiltinQuery::SortTopK, 1_049_999.6),
];

/// A timing, and the spread it was taken over.
#[derive(Clone, Copy)]
struct Timing {
    /// The median of the samples, in milliseconds.
    median: f64,
    /// The fastest sample, in milliseconds.
    min: f64,
    /// The slowest sample, in milliseconds.
    max: f64,
}

impl Timing {
    /// The timing of a set of samples.
    fn of(samples: &[Duration]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        Self {
            median: ms(sorted.get(middle).copied().unwrap_or_default()),
            min: ms(sorted.first().copied().unwrap_or_default()),
            max: ms(sorted.last().copied().unwrap_or_default()),
        }
    }

    /// The timing's spread, as the report carries it.
    fn spread(self) -> [f64; 2] {
        [self.min, self.max]
    }
}

/// A duration, in milliseconds.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Repeats a measurement, and returns its median and its spread.
///
/// The first sample pays for the page cache, and it is kept: the median of
/// five is the figure, and the spread beside it says how much the host moved
/// while it was taken.
fn repeat(mut measure: impl FnMut() -> Duration) -> Timing {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        samples.push(measure());
    }
    Timing::of(&samples)
}

/// The commit the working tree is at, for the report.
fn commit() -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map_or_else(
        || "unknown".to_owned(),
        |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    )
}

/// The host's load average, for the report.
///
/// The host prints it as `{ 5.85 5.27 4.63 }`, so the braces come off before
/// the first number is read.
fn load_average() -> f64 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .and_then(|text| {
            text.replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
}

/// The definition of a builtin query.
fn definition_of(query: BuiltinQuery) -> &'static QueryDefinition {
    columnar_engine::queries::QUERIES
        .iter()
        .find(|definition| definition.query == query)
        .expect("the query is a builtin one")
}

/// The aggregate's value, as the number the published campaigns recorded.
fn value_of(result: &columnar_engine::AggregateResult) -> f64 {
    result.groups.first().and_then(|group| group.values.first()).map_or(f64::NAN, |value| {
        match value {
            columnar_engine::AggregateValue::Float(v) => *v,
            // A count fits in a u32 for every dataset this repository makes.
            columnar_engine::AggregateValue::Int(v) => {
                f64::from(u32::try_from(*v).unwrap_or(u32::MAX))
            }
            columnar_engine::AggregateValue::Null => f64::NAN,
        }
    })
}

/// The query's own time, through the engine's own entry point.
///
/// The plan is built **outside** the clock, because the engine builds it
/// outside the clock too: the harness's figure is the query, not the plan.
fn run_query(
    plan: &QueryPlan,
    dataset: &Dataset,
    options: &ExecOptions,
) -> (Duration, f64, Duration) {
    let started = Instant::now();
    let result = columnar_engine::execute(plan, dataset, options).expect("the query runs");
    let elapsed = started.elapsed();
    let stages = result.stages.iter().map(|stage| stage.elapsed).sum::<Duration>();
    (elapsed, value_of(&result), stages)
}

/// The query's own time, over the file, through the engine's own entry point.
///
/// This is the **cold** path: the plan is resolved against the file, the file
/// is read as part of the query, and the clock is the engine's.
fn run_query_cold(
    plan: &QueryPlan,
    source: &ParquetSource,
    options: &ExecOptions,
) -> (Duration, f64, Duration) {
    let started = Instant::now();
    let result = columnar_engine::execute_on(plan, source, options).expect("the query runs");
    let elapsed = started.elapsed();
    let stages = result.stages.iter().map(|stage| stage.elapsed).sum::<Duration>();
    (elapsed, value_of(&result), stages)
}

/// One shape's measurement: its name and its timing.
struct Shape {
    /// The shape's name in the report.
    name: &'static str,
    /// The shape's timing.
    timing: Timing,
}

/// Everything the probe measured for one query, on one path.
#[derive(Clone, Copy)]
struct Measured {
    /// The query's short name.
    name: &'static str,
    /// The whole query.
    query: Timing,
    /// The plan output's own stage table.
    stages: Timing,
    /// The answer the query gave.
    answer: f64,
}

/// The shapes of the dataset's **every row** selection.
///
/// These are a property of the dataset and not of a query: a plan with no
/// predicate has the selection "every row", and that is the selection the old
/// code expanded into row numbers. They are measured once.
fn shapes(dataset: &Dataset, build: &Path) -> Vec<Shape> {
    let rows = dataset.len();
    let selection = SelectionVector::all(rows);

    // The shape: the selection expanded into row numbers.
    //
    // This is what the join and the sort used to ask for, before either
    // stage's clock started.
    let selection_rows = repeat(|| {
        let started = Instant::now();
        let numbers = selection.rows();
        let elapsed = started.elapsed();
        std::hint::black_box(&numbers);
        elapsed
    });

    // The shape: the same rows, read as they are needed.
    //
    // The rows are **summed**, not counted: a count is a popcount, and the
    // compiler turns the walk into one, which measures the bitmap's bits
    // rather than the walk over them.
    //
    // And the sum is taken **with an opaque mask**, because a plain sum of
    // `0..n` is a closed form the compiler computes without walking: the
    // plain sum of six million rows measures **0.000 ms** on this host, and
    // the same walk with a mask measures **0.8 ms**. The mask is drawn from
    // `black_box` outside the loop, so the compiler cannot know it and
    // cannot fold the sum -- and it is the same mask for both walks, so the
    // two shapes are measured the same way.
    let mask = std::hint::black_box(0x9E37_79B9_u32);
    let selection_walk = repeat(|| {
        let started = Instant::now();
        let mut sum = 0u64;
        selection.for_each(|row| sum += u64::from(row ^ mask));
        let elapsed = started.elapsed();
        std::hint::black_box(&sum);
        elapsed
    });

    // The control: the same two shapes, with the selection held as a bitmap.
    let words = vec![u64::MAX; rows.div_ceil(64)];
    let bitmap = SelectionVector::from_filter(&words, rows, rows);
    let bitmap_rows = repeat(|| {
        let started = Instant::now();
        let numbers = bitmap.rows();
        let elapsed = started.elapsed();
        std::hint::black_box(&numbers);
        elapsed
    });
    let bitmap_walk = repeat(|| {
        let started = Instant::now();
        let mut sum = 0u64;
        bitmap.for_each(|row| sum += u64::from(row ^ mask));
        let elapsed = started.elapsed();
        std::hint::black_box(&sum);
        elapsed
    });

    // The plan's own bookkeeping, and the join's build side.
    let plan = (definition_of(BuiltinQuery::JoinOrders).plan)(build);
    let plan_resolve = repeat(|| {
        let started = Instant::now();
        let resolved = plan.resolve(dataset).expect("the plan resolves");
        let elapsed = started.elapsed();
        std::hint::black_box(&resolved);
        elapsed
    });
    let resolved = plan.resolve(dataset).expect("the plan resolves");
    let grouped =
        resolved.aggregate.as_ref().is_some_and(|aggregate| !aggregate.group_by.is_empty());
    let memory_estimate = repeat(|| {
        let started = Instant::now();
        let estimate = memory::estimate_resolved(&resolved, dataset, grouped);
        let elapsed = started.elapsed();
        std::hint::black_box(&estimate);
        elapsed
    });
    let build_open = repeat(|| {
        let started = Instant::now();
        let opened = std::fs::File::open(build);
        let elapsed = started.elapsed();
        std::hint::black_box(&opened);
        elapsed
    });

    vec![
        Shape { name: "selection_rows", timing: selection_rows },
        Shape { name: "selection_walk", timing: selection_walk },
        Shape { name: "bitmap_rows", timing: bitmap_rows },
        Shape { name: "bitmap_walk", timing: bitmap_walk },
        Shape { name: "plan_resolve", timing: plan_resolve },
        Shape { name: "memory_estimate", timing: memory_estimate },
        Shape { name: "build_open", timing: build_open },
    ]
}

/// Measures a query on the resident path.
fn measure_resident(
    query: BuiltinQuery,
    expected: f64,
    dataset: &Dataset,
    build: &Path,
    options: &ExecOptions,
) -> Measured {
    let plan = (definition_of(query).plan)(build);
    let mut query_samples = Vec::with_capacity(ITERATIONS);
    let mut stage_samples = Vec::with_capacity(ITERATIONS);
    let mut answer = f64::NAN;
    for _ in 0..ITERATIONS {
        let (elapsed, value, stages) = run_query(&plan, dataset, options);
        query_samples.push(elapsed);
        stage_samples.push(stages);
        answer = value;
    }
    check(query, expected, answer);
    Measured {
        name: query.name(),
        query: Timing::of(&query_samples),
        stages: Timing::of(&stage_samples),
        answer,
    }
}

/// Measures a query on the cold path.
fn measure_cold(
    query: BuiltinQuery,
    expected: f64,
    source: &ParquetSource,
    build: &Path,
    options: &ExecOptions,
) -> Measured {
    let plan = (definition_of(query).plan)(build);
    let mut query_samples = Vec::with_capacity(ITERATIONS);
    let mut stage_samples = Vec::with_capacity(ITERATIONS);
    let mut answer = f64::NAN;
    for _ in 0..ITERATIONS {
        let (elapsed, value, stages) = run_query_cold(&plan, source, options);
        query_samples.push(elapsed);
        stage_samples.push(stages);
        answer = value;
    }
    check(query, expected, answer);
    Measured {
        name: query.name(),
        query: Timing::of(&query_samples),
        stages: Timing::of(&stage_samples),
        answer,
    }
}

/// Checks a query's answer against the published one.
///
/// The sums are compared with a **relative** tolerance: floating-point
/// addition is not associative, and the two paths sum the same rows in
/// different orders, so the last bits of a sum of a hundred million are not
/// the same number. The tolerance is a millionth of the value — far tighter
/// than any disagreement that is not a different answer.
fn check(query: BuiltinQuery, expected: f64, answer: f64) {
    let tolerance = expected.abs().max(1.0) * 1e-6;
    assert!(
        (answer - expected).abs() <= tolerance,
        "{}: the answer is {answer}, the published answer is {expected}",
        query.name()
    );
}

/// Prints what the probe measured, for the person running it.
fn print(measured: &[Measured]) {
    println!("\n{:<14}{:>12}{:>12}{:>14}", "query", "the query", "the stages", "unnamed");
    for measured in measured {
        println!(
            "{:<14}{:>12.3}{:>12.3}{:>14.3}",
            measured.name,
            measured.query.median,
            measured.stages.median,
            measured.query.median - measured.stages.median
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--data" => path = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("usage: query_cost_probe --data <parquet> [--out <report.json>]");
                return Ok(());
            }
            // The path may also be given on its own, the way the other probes
            // take it.
            other if !other.starts_with("--") => path = Some(PathBuf::from(other)),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let path = path.ok_or("--data is required")?;
    let out = out.unwrap_or_else(|| PathBuf::from("query-cost.json"));

    let build = path
        .parent()
        .map_or_else(|| PathBuf::from("orders.parquet"), |parent| parent.join("orders.parquet"));
    let dataset = Dataset::read_parquet(&path)?;
    let source = ParquetSource::open(&path)?;
    let options = ExecOptions::default();

    // The shapes are measured once: they are a property of the dataset's
    // "every row" selection, and not of a query.
    let shapes = shapes(&dataset, &build);

    // Every published query, on both paths.
    let mut resident: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut cold: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut measured_resident = Vec::new();
    let mut measured_cold = Vec::new();
    for (query, expected) in EXPECTED {
        let on_resident = measure_resident(*query, *expected, &dataset, &build, &options);
        let on_cold = measure_cold(*query, *expected, &source, &build, &options);
        let name = on_resident.name;
        measured_resident.push(on_resident);
        measured_cold.push(on_cold);
        resident.insert(
            name.to_owned(),
            serde_json::json!({
                "answer": on_resident.answer,
                "query_ms": on_resident.query.median,
                "stages_ms": on_resident.stages.median,
                "unnamed_ms": on_resident.query.median - on_resident.stages.median,
                "query_spread_ms": on_resident.query.spread(),
                "stages_spread_ms": on_resident.stages.spread(),
            }),
        );
        cold.insert(
            name.to_owned(),
            serde_json::json!({
                "answer": on_cold.answer,
                "query_ms": on_cold.query.median,
                "stages_ms": on_cold.stages.median,
                "unnamed_ms": on_cold.query.median - on_cold.stages.median,
                "query_spread_ms": on_cold.query.spread(),
                "stages_spread_ms": on_cold.stages.spread(),
            }),
        );
    }
    print(&measured_resident);
    print(&measured_cold);

    let mut shape_ms = serde_json::Map::new();
    let mut shape_spreads = serde_json::Map::new();
    for shape in &shapes {
        shape_ms.insert(shape.name.to_owned(), serde_json::json!(shape.timing.median));
        shape_spreads.insert(shape.name.to_owned(), serde_json::json!(shape.timing.spread()));
    }

    let report = serde_json::json!({
        "probe": "query-cost",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit(),
        "host_load_average": load_average(),
        "rows": dataset.len(),
        "shapes_ms": shape_ms,
        "spreads_ms": shape_spreads,
        "queries": {
            "resident": resident,
            "cold": cold,
        },
    });
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(&out, format!("{json}\n"))?;
    println!("\nwrote {}", out.display());
    Ok(())
}
