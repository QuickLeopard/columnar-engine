//! The engine's own rows, through the store.
//!
//! ```text
//! cargo run --release -p columnar-store --example engine_rows_probe -- \
//!     data/lineitem.parquet --out bench-results/engine-rows-run1.json
//! ```
//!
//! # The question
//!
//! The store's claim so far is that **it reproduces the published file byte
//! for byte** from the file's own rows, read through Arrow. Spec 042 asks the
//! harder question: **what happens when the rows come from the engine?**
//!
//! The engine's rows are the engine's own vocabulary -- a `ColumnChunk` with
//! an `Encoding` and a validity mask. Arrow's are Arrow's. Somebody has to
//! turn one into the other, and that somebody's **cost**, and the **file it
//! produces**, are the two things this probe measures.
//!
//! # The two shapes
//!
//! | | |
//! |---|---|
//! | **the file's own rows** | the engine's eager read of the published file: the rows as they lie, dictionaries and all |
//! | **a plan's rows** | the rows that survive a filter, gathered by the engine's own projection: plain columns, no dictionary |
//!
//! The second shape is **a different file by construction**: the projection
//! materialises the dictionary away, so the values are the same and the bytes
//! are not. Both shapes are measured, and both files are compared with the
//! published one.
//!
//! # What the probe measures
//!
//! | | |
//! |---|---|
//! | **`engine_read`** | the engine's eager read: the file in, the chunks out |
//! | **`convert`** | the conversion: the engine's vocabulary to Arrow's |
//! | **`append`** | the store's own API: the rows in, the file out, the commit |
//! | **`arrow_path`** | the store's own Arrow path, for comparison: Arrow's rows in, the same file out |
//!
//! And the bytes: whether the engine's rows write **the published file**, and
//! if they do not, **where the two first differ**.
//!
//! # Three files, three questions
//!
//! 1. **The store's Arrow path** writes the published file byte for byte.
//!    That is the store's claim, and it is the reference every other file is
//!    compared with.
//! 2. **The store's conversion, under the published file's own schema**,
//!    writes the published file byte for byte too. That says the *rows* are
//!    the same, and that the conversion is exact.
//! 3. **The store's own `append_rows`** writes the engine's rows under the
//!    *store's* schema rule -- a column that holds no NULL is written
//!    REQUIRED -- and that is a different file, because the engine's
//!    vocabulary says which rows are NULL and not whether a column may be.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::array::RecordBatchReader;
use arrow::record_batch::RecordBatch;
use columnar_engine::{ComparisonOp, Dataset, Predicate, QueryPlan, ScalarValue};
use columnar_store::{writer_properties, Store, BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// The one column the published file writes plain: it is the high-cardinality
/// one, and the file's own policy is the policy this probe compares against.
const COMMENT_COLUMN: &str = "l_comment";

/// How many times each measurement is repeated.
const ITERATIONS: usize = 3;

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

/// The commit the working tree is at, for the report.
fn commit_sha() -> String {
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

/// The file's own schema, read from the file.
///
/// The names, the types, the nullability and the metadata: a schema built by
/// hand would be a different schema, and a different schema is a different
/// file.
fn source_schema(path: &Path) -> arrow::datatypes::SchemaRef {
    let file = File::open(path).expect("the source opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    builder.build().expect("the reader").schema()
}

/// The file's own rows, read once, in the store's grid.
fn read_source(path: &Path) -> Vec<RecordBatch> {
    let file = File::open(path).expect("the source opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let reader = builder.with_batch_size(BATCH_ROWS).build().expect("the reader");
    reader.map(|batch| batch.expect("the batch")).collect()
}

/// Compares two files byte for byte, and says where they first differ.
fn compare_files(left: &Path, right: &Path) -> (bool, Option<u64>) {
    let mut left_reader = BufReader::new(File::open(left).expect("the left file opens"));
    let mut right_reader = BufReader::new(File::open(right).expect("the right file opens"));
    let mut left_chunk = vec![0u8; 64 * 1024];
    let mut right_chunk = vec![0u8; 64 * 1024];
    let mut offset = 0u64;
    loop {
        let left_read = left_reader.read(&mut left_chunk).expect("the left file reads");
        let right_read = right_reader.read(&mut right_chunk).expect("the right file reads");
        if left_read != right_read {
            return (false, Some(offset));
        }
        if left_read == 0 {
            return (true, None);
        }
        if left_chunk[..left_read] != right_chunk[..right_read] {
            let differing = left_chunk
                .iter()
                .zip(right_chunk.iter())
                .position(|(left, right)| left != right)
                .unwrap_or(0);
            return (false, Some(offset + differing as u64));
        }
        offset += left_read as u64;
    }
}

/// The file's size in bytes.
fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// The plan the probe's second shape runs: the **Q6** filter.
///
/// The plan has **no aggregate**: the aggregate folds the rows into groups,
/// and there would be nothing left to write.
fn q6_plan() -> QueryPlan {
    QueryPlan {
        filter: vec![
            Predicate::new(
                "l_shipdate",
                ComparisonOp::GtEq,
                ScalarValue::Utf8("1994-01-01".to_owned()),
            ),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::Lt,
                ScalarValue::Utf8("1995-01-01".to_owned()),
            ),
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05)),
            Predicate::new("l_discount", ComparisonOp::LtEq, ScalarValue::Float64(0.07)),
            Predicate::new("l_quantity", ComparisonOp::Lt, ScalarValue::Float64(25.0)),
        ],
        projection: vec!["l_orderkey".to_owned(), "l_shipdate".to_owned(), "l_discount".to_owned()],
        aggregate: None,
        join: None,
        sort: None,
    }
}

/// What one pass of the write path produced.
struct Pass {
    /// Its rows.
    rows: usize,
    /// Its bytes.
    bytes: u64,
    /// Whether the file equals the published file.
    identical: bool,
    /// Where the two first differ.
    first_difference: Option<u64>,
}

/// The engine's own rows, through the store's own API, once.
fn engine_pass(path: &Path, store: &Store, table: &str) -> (Pass, Duration, Duration, Duration) {
    let read_started = Instant::now();
    let dataset = Dataset::read_parquet(path).expect("the engine reads the file");
    let read = read_started.elapsed();

    let convert_started = Instant::now();
    let _ = columnar_store::rows::arrays(&dataset.chunks).expect("the chunks become arrays");
    let convert = convert_started.elapsed();

    let append_started = Instant::now();
    let manifest = store
        .append_rows(table, &dataset.chunks, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let append = append_started.elapsed();

    let written = store.file_path(table, &manifest.files.last().expect("a file").name);
    let (identical, first_difference) = compare_files(path, &written);
    let bytes = file_bytes(&written);
    (Pass { rows: dataset.rows, bytes, identical, first_difference }, read, convert, append)
}

/// The store's conversion, under the published file's own schema.
fn converted_pass(
    path: &Path,
    schema: &arrow::datatypes::SchemaRef,
    store: &Store,
    table: &str,
) -> Pass {
    let dataset = Dataset::read_parquet(path).expect("the engine reads the file");
    let arrays = columnar_store::rows::arrays(&dataset.chunks).expect("the arrays");
    let mut batches = Vec::new();
    let mut start = 0;
    while start < dataset.rows {
        let len = BATCH_ROWS.min(dataset.rows - start);
        let columns = arrays.iter().map(|array| array.slice(start, len)).collect();
        batches.push(RecordBatch::try_new(schema.clone(), columns).expect("the batch"));
        start += len;
    }
    let manifest = store
        .append_with(table, &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let written = store.file_path(table, &manifest.files.last().expect("a file").name);
    let (identical, first_difference) = compare_files(path, &written);
    let bytes = file_bytes(&written);
    Pass { rows: dataset.rows, bytes, identical, first_difference }
}

/// The store's own Arrow path, for comparison.
fn arrow_pass(path: &Path, store: &Store, table: &str) -> Pass {
    let batches = read_source(path);
    let manifest = store
        .append_with(table, &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let written = store.file_path(table, &manifest.files.last().expect("a file").name);
    let (identical, first_difference) = compare_files(path, &written);
    let bytes = file_bytes(&written);
    let rows = usize::try_from(manifest.rows).expect("the rows fit in this platform's words");
    Pass { rows, bytes, identical, first_difference }
}

/// The engine's own rows, through the store's own API, repeated.
fn measure_engine(
    path: &Path,
    store: &Store,
    table: &str,
) -> (Vec<Duration>, Vec<Duration>, Vec<Duration>, Pass) {
    let mut reads = Vec::with_capacity(ITERATIONS);
    let mut converts = Vec::with_capacity(ITERATIONS);
    let mut appends = Vec::with_capacity(ITERATIONS);
    let mut last = None;
    for _ in 0..ITERATIONS {
        let (pass, read, convert, append) = engine_pass(path, store, table);
        reads.push(read);
        converts.push(convert);
        appends.push(append);
        last = Some(pass);
    }
    let pass = last.expect("the probe ran at least once");
    (reads, converts, appends, pass)
}

/// The probe's arguments.
struct Arguments {
    /// The file the probe reads and writes.
    file: PathBuf,
    /// Where the report goes.
    out: Option<String>,
    /// Where the store goes.
    work_dir: PathBuf,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut arguments = std::env::args().skip(1);
    let mut file = PathBuf::from("data/lineitem.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/engine-rows-probe");
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--file" => {
                if let Some(value) = arguments.next() {
                    file = PathBuf::from(value);
                }
            }
            "--out" => out = arguments.next(),
            "--work-dir" => {
                if let Some(value) = arguments.next() {
                    work_dir = PathBuf::from(value);
                }
            }
            _ => {}
        }
    }
    Arguments { file, out, work_dir }
}

/// Prints a set of measurements, with their spreads.
fn print_timings(medians: &serde_json::Map<String, serde_json::Value>, names: &[&str]) {
    for name in names {
        let median = medians.get(*name).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        println!("{name:<18}: {median:>10.3} ms");
    }
}

fn main() {
    let arguments = arguments();
    let path = arguments.file.clone();
    let work_dir = arguments.work_dir.clone();

    println!("file: {}", path.display());
    println!("grid: {BATCH_ROWS} rows");
    println!("iterations: {ITERATIONS}\n");

    std::fs::create_dir_all(&work_dir).expect("the work directory is created");
    let schema = source_schema(&path);

    // 1. The engine's own rows, through the store's own API.
    let engine_store = Store::open(work_dir.join("engine")).expect("the store opens");
    let (reads, converts, appends, engine) = measure_engine(&path, &engine_store, "lineitem");

    // 2. The store's conversion, under the published file's own schema.
    let converted_store = Store::open(work_dir.join("converted")).expect("the store opens");
    let converted = converted_pass(&path, &schema, &converted_store, "lineitem");

    // 3. The store's own Arrow path, for comparison.
    let arrow_store = Store::open(work_dir.join("arrow")).expect("the store opens");
    let arrow_started = Instant::now();
    let arrow = arrow_pass(&path, &arrow_store, "lineitem");
    let arrow_append = arrow_started.elapsed();

    // 4. A plan's rows: the survivors, gathered, plain.
    let source = columnar_engine::open_source(&path).expect("the source opens");
    let plan_started = Instant::now();
    let rows = columnar_engine::execute_rows_on(
        &q6_plan(),
        &source,
        &columnar_engine::ExecOptions::default(),
    )
    .expect("the plan runs");
    let plan_read = plan_started.elapsed();
    let plan_store = Store::open(work_dir.join("plan")).expect("the store opens");
    let plan_started = Instant::now();
    let plan_manifest = plan_store
        .append_rows("survivors", &rows.chunks, writer_properties(&[]))
        .expect("the rows are written");
    let plan_append = plan_started.elapsed();
    let plan_path =
        plan_store.file_path("survivors", &plan_manifest.files.last().expect("a file").name);

    let read_timing = Timing::of(&reads);
    let convert_timing = Timing::of(&converts);
    let append_timing = Timing::of(&appends);

    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    let mut put = |name: &str, timing: &Timing| {
        medians.insert(name.to_owned(), serde_json::json!(timing.median));
        spreads.insert(name.to_owned(), serde_json::json!([timing.min, timing.max]));
    };
    put("engine_read", &read_timing);
    put("convert", &convert_timing);
    put("append", &append_timing);
    medians.insert("arrow_path".to_owned(), serde_json::json!(ms(arrow_append)));
    spreads
        .insert("arrow_path".to_owned(), serde_json::json!([ms(arrow_append), ms(arrow_append)]));
    medians.insert("plan_read".to_owned(), serde_json::json!(ms(plan_read)));
    spreads.insert("plan_read".to_owned(), serde_json::json!([ms(plan_read), ms(plan_read)]));
    medians.insert("plan_append".to_owned(), serde_json::json!(ms(plan_append)));
    spreads.insert("plan_append".to_owned(), serde_json::json!([ms(plan_append), ms(plan_append)]));

    print_the_measurements(
        &medians,
        &path,
        &engine,
        &converted,
        &arrow,
        file_bytes(&plan_path),
        rows.rows,
    );

    let report = report(
        &path,
        &engine,
        &converted,
        &arrow,
        file_bytes(&plan_path),
        rows.rows,
        &medians,
        &spreads,
    );
    println!();
    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&work_dir).ok();
}

/// What the probe measured, printed.
#[allow(clippy::too_many_arguments)]
fn print_the_measurements(
    medians: &serde_json::Map<String, serde_json::Value>,
    path: &Path,
    engine: &Pass,
    converted: &Pass,
    arrow: &Pass,
    plan_bytes: u64,
    plan_rows: usize,
) {
    println!("--- the timings ---");
    print_timings(
        medians,
        &["engine_read", "convert", "append", "arrow_path", "plan_read", "plan_append"],
    );
    println!();
    println!("--- the bytes ---");
    println!("published:            {} bytes", file_bytes(path));
    println!("the engine's rows:    {} bytes ({} rows)", engine.bytes, engine.rows);
    println!("the conversion alone: {} bytes ({} rows)", converted.bytes, converted.rows);
    println!("arrow's rows:         {} bytes ({} rows)", arrow.bytes, arrow.rows);
    println!("a plan's rows:        {plan_bytes} bytes ({plan_rows} rows)");
    println!(
        "the engine's rows identical:    {}{}",
        engine.identical,
        engine
            .first_difference
            .map_or_else(String::new, |offset| format!(", first difference at {offset}"))
    );
    println!(
        "the conversion alone identical: {}{}",
        converted.identical,
        converted
            .first_difference
            .map_or_else(String::new, |offset| format!(", first difference at {offset}"))
    );
    println!(
        "arrow's rows identical:         {}{}",
        arrow.identical,
        arrow
            .first_difference
            .map_or_else(String::new, |offset| format!(", first difference at {offset}"))
    );
}

/// The report, for the committed raw report.
#[allow(clippy::too_many_arguments)]
fn report(
    path: &Path,
    engine: &Pass,
    converted: &Pass,
    arrow: &Pass,
    plan_bytes: u64,
    plan_rows: usize,
    medians: &serde_json::Map<String, serde_json::Value>,
    spreads: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "probe": "engine-rows",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "batch_rows": BATCH_ROWS,
        "bytes_published": file_bytes(path),
        "bytes_engine_rows": engine.bytes,
        "rows_engine_rows": engine.rows,
        "bytes_converted_rows": converted.bytes,
        "bytes_arrow_rows": arrow.bytes,
        "bytes_plan_rows": plan_bytes,
        "rows_plan_rows": plan_rows,
        "identical_to_published": engine.identical,
        "first_difference": engine.first_difference,
        "converted_identical_to_published": converted.identical,
        "converted_first_difference": converted.first_difference,
        "arrow_identical_to_published": arrow.identical,
        "arrow_first_difference": arrow.first_difference,
        "measurements_ms": medians,
        "spreads_ms": spreads,
    })
}
