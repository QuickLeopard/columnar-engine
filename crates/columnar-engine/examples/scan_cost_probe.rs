//! The scan, taken apart.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind spec 023's next question: **where does the scan's time go?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example scan_cost_probe -- \
//!     data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! The engine's cold `join-orders` is **100.4 ms**, against `DuckDB`'s
//! **93.0 ms**, and the plan output says where the time is: the **scan** is
//! **71.4 ms** of that query, and the join stage is **21.9 ms** of it. So the
//! remaining gap to `DuckDB` is in the **scan**, not in the join -- the join's
//! own probe and the sort's own probe were both taken apart, and the scan
//! never was.
//!
//! The scan reads two columns of `lineitem` and materialises both:
//! `l_orderkey`, the join key, and `l_extendedprice`, the column the revenue
//! is computed from. So the probe measures the scan in the shapes it is made
//! of:
//!
//! | | |
//! |---|---|
//! | **`crate_orderkey`** | the parquet crate's read of `l_orderkey` alone |
//! | **`crate_price`** | the parquet crate's read of `l_extendedprice` alone |
//! | **`crate_both`** | the crate's read of both columns |
//! | **`engine_orderkey`** | the engine's own scan, materialising `l_orderkey` |
//! | **`engine_price`** | the engine's own scan, materialising `l_extendedprice` |
//! | **`engine_both`** | the engine's own scan, materialising both columns |
//! | **`crate_orders_both`** | the crate's read of `orders.parquet`'s two columns |
//!
//! The crate's shapes are the crate's own reader, with a projection mask, no
//! row filter and no pushdown: the batches are counted and dropped. The
//! engine's shapes are `ParquetSource::read` itself, which is what the lazy
//! scan runs. The difference between a crate shape and the engine shape over
//! the same columns is the engine's own work, and the last shape is the
//! join's **build side** -- read inside the join stage's own clock, and never
//! seen by the scan stage at all.
//!
//! # The shapes are the engine's shapes
//!
//! Every shape is read at the **file's own batch size**: the engine's
//! derived one, asked of the engine itself rather than copied out of it. For
//! the published `lineitem` file that is **125,000 rows** -- its row groups
//! hold a million rows each, and 125,000 is the largest divisor of a million
//! that fits under the engine's limit. So the crate's read and the engine's
//! read are the same shape, and the difference between them is the engine's
//! work and nothing else.
//!
//! # What the figures are not
//!
//! These are **cold** figures on a **shared** host, taken five times each and
//! reported as the median with the spread beside it. The report carries the
//! host's load average, so a figure that drifted can be read against it.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_engine::{ParquetSource, PushdownPlan, MAX_READER_BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The file's root index of `l_orderkey`.
const ORDERKEY: usize = 0;
/// The file's root index of `l_extendedprice`.
const EXTENDEDPRICE: usize = 5;
/// The orders file's root index of `o_orderkey`.
const ORDERS_ORDERKEY: usize = 0;
/// The orders file's root index of `o_orderdate`.
const ORDERS_ORDERDATE: usize = 4;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// A measurement: its median, and the spread around it.
#[derive(Debug, Clone, Copy)]
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
        Self { median: ms(sorted[middle]), min: ms(sorted[0]), max: ms(sorted[sorted.len() - 1]) }
    }
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

/// The parquet crate's read of some columns, on its own.
///
/// The reader, with a projection mask and **no** row filter: the batches are
/// counted and dropped. This is the crate's share of the scan's cost, with
/// none of the engine's work in it.
fn crate_read(path: &Path, columns: &[usize], batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(batch_rows)
        .build()
        .expect("the reader builds");
    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        rows += batch.num_rows();
    }
    (started.elapsed(), rows)
}

/// The engine's own scan, materialising some columns.
///
/// This is the scan the lazy path runs. The source is opened **outside** the
/// clock, exactly as the engine's own plan output times the scan stage, and
/// the read itself is inside it.
fn engine_scan(path: &Path, columns: &[usize]) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source
        .read(columns, &PushdownPlan::new(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The batch size the engine's own read uses on a file.
///
/// The engine derives it from the file: the greatest common divisor of the
/// row groups' row counts, rounded down to a divisor that fits under the
/// limit. The probe asks the engine for it, so that the probe's shapes are
/// the engine's shapes by construction rather than by a copy of the rule.
fn batch_rows(path: &Path) -> usize {
    let source = ParquetSource::open(path).expect("the source opens");
    source.batch_rows_under(MAX_READER_BATCH_ROWS)
}

/// The file's row-group count.
fn row_groups(path: &Path) -> usize {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    reader.metadata().num_row_groups()
}

/// Which part of the query a shape belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The crate's read, on its own.
    Crate,
    /// The engine's own scan.
    Engine,
    /// The join's build side.
    Build,
}

impl Part {
    /// The part's heading, for the printed table.
    fn heading(self) -> &'static str {
        match self {
            Self::Crate => "--- the parquet crate's read, on its own ---",
            Self::Engine => "--- the engine's own scan ---",
            Self::Build => "--- the join's build side, read inside the join stage ---",
        }
    }
}

/// One shape the probe measures.
struct Shape {
    /// The shape's name in the report.
    name: &'static str,
    /// The shape's columns, for the printed table.
    label: &'static str,
    /// Which part of the query the shape belongs to.
    part: Part,
    /// The file the shape reads.
    path: PathBuf,
    /// The file's root indices the shape reads.
    columns: Vec<usize>,
    /// The batch size the shape's file is read at.
    batch_rows: usize,
    /// Whether the shape goes through the engine's own scan.
    engine: bool,
}

/// Measures one shape: its median, its spread, and how many rows it read.
fn measure(shape: &Shape) -> (Timing, usize) {
    let mut rows = 0usize;
    let timing = repeat(|| {
        let (elapsed, read) = if shape.engine {
            engine_scan(&shape.path, &shape.columns)
        } else {
            crate_read(&shape.path, &shape.columns, shape.batch_rows)
        };
        rows = read;
        elapsed
    });
    (timing, rows)
}

/// The shapes the probe measures, in the order the table prints them.
fn shapes(lineitem: &Path, orders: &Path, lineitem_rows: usize, orders_rows: usize) -> Vec<Shape> {
    let both = vec![ORDERKEY, EXTENDEDPRICE];
    vec![
        Shape {
            name: "crate_orderkey",
            label: "l_orderkey only",
            part: Part::Crate,
            path: lineitem.to_path_buf(),
            columns: vec![ORDERKEY],
            batch_rows: lineitem_rows,
            engine: false,
        },
        Shape {
            name: "crate_price",
            label: "l_extendedprice only",
            part: Part::Crate,
            path: lineitem.to_path_buf(),
            columns: vec![EXTENDEDPRICE],
            batch_rows: lineitem_rows,
            engine: false,
        },
        Shape {
            name: "crate_both",
            label: "both columns",
            part: Part::Crate,
            path: lineitem.to_path_buf(),
            columns: both.clone(),
            batch_rows: lineitem_rows,
            engine: false,
        },
        Shape {
            name: "engine_orderkey",
            label: "l_orderkey only",
            part: Part::Engine,
            path: lineitem.to_path_buf(),
            columns: vec![ORDERKEY],
            batch_rows: lineitem_rows,
            engine: true,
        },
        Shape {
            name: "engine_price",
            label: "l_extendedprice only",
            part: Part::Engine,
            path: lineitem.to_path_buf(),
            columns: vec![EXTENDEDPRICE],
            batch_rows: lineitem_rows,
            engine: true,
        },
        Shape {
            name: "engine_both",
            label: "both columns",
            part: Part::Engine,
            path: lineitem.to_path_buf(),
            columns: both,
            batch_rows: lineitem_rows,
            engine: true,
        },
        Shape {
            name: "crate_orders_both",
            label: "o_orderkey, o_orderdate",
            part: Part::Build,
            path: orders.to_path_buf(),
            columns: vec![ORDERS_ORDERKEY, ORDERS_ORDERDATE],
            batch_rows: orders_rows,
            engine: false,
        },
    ]
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = PathBuf::from(path);
    let mut out_path: Option<String> = None;
    while let Some(argument) = arguments.next() {
        if argument == "--out" {
            out_path = arguments.next();
        }
    }

    // The join's build side is the file beside the probe's file.
    let orders = path.parent().unwrap_or_else(|| Path::new(".")).join("orders.parquet");
    let lineitem_rows = batch_rows(&path);
    let orders_rows = batch_rows(&orders);
    let shapes = shapes(&path, &orders, lineitem_rows, orders_rows);

    println!("file: {}", path.display());
    println!("row groups: {}, batch rows: {lineitem_rows}", row_groups(&path));
    println!("iterations: {ITERATIONS}\n");

    let mut measurements = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    let mut printed: Option<Part> = None;

    for shape in &shapes {
        if printed != Some(shape.part) {
            println!("{}", shape.part.heading());
            printed = Some(shape.part);
        }
        let (timing, rows) = measure(shape);
        println!(
            "{:<18} {:<22}: {:8.2} ms  [{:7.2} .. {:7.2}]  {rows} rows",
            shape.name, shape.label, timing.median, timing.min, timing.max
        );
        measurements.insert(shape.name.to_owned(), serde_json::json!(timing.median));
        spreads.insert(shape.name.to_owned(), serde_json::json!([timing.min, timing.max]));
    }
    println!();

    let report = serde_json::json!({
        "probe": "scan-cost",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit(),
        "host_load_average": load_average(),
        "batch_rows": lineitem_rows,
        "row_groups": row_groups(&path),
        "measurements_ms": measurements,
        "spreads_ms": spreads,
    });

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = out_path {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }
}
