//! The file boundary's cost: the same rows, read as one file and as six.
//!
//! ```text
//! cargo run --release -p columnar-engine --example many_files_probe -- \
//!     data/lineitem.parquet --out bench-results/many-files-run1.json
//! ```
//!
//! # The question
//!
//! The store writes a table as a **set of files** -- a commit appends one and never modifies an old one
//! -- and the manifest names them. **The engine's only source is one file**, so a store's table cannot be
//! read by the engine today, and the embedder's only way to read it is to open each file and run the
//! query over each one, which is **wrong three times over**: the rows of a filtered table are split
//! across the files, the groups of an aggregate are split across them, and the sides of a join are split
//! across them.
//!
//! So the engine needs a source over a set of files. **And before it builds one, the cost of the file
//! boundary has to be measured** -- because another file is another open, another footer, another metadata
//! parse, and the source's shape is decided by what that costs, not by what it feels like.
//!
//! # The experiment
//!
//! **The published file's six row groups are the same rows as the published file.** So they are the
//! natural experiment: the probe writes them as **six files**, and then reads
//!
//! - **the one file** -- the published file, through the engine's own reader;
//! - **the six files** -- each through the engine's own reader, and the sum.
//!
//
//! **The only difference between the two reads is the number of files.** The rows are the same, the
//! batches are the same, the columns are the same, and the host is the same.
//!
//! **And the sum is the upper bound.** A source that reads the set as one table would merge the metadata
//! and cross the file boundary in its batches, so it would cost **less** than the sum. **The gap between
//! the sum and the one file's read is what the source could win** -- and it is the reason the source is
//! worth building, or the reason it is not.
//!
//! # The answers are checked
//!
//! The six files' rows are compared with the one file's, **row by row**, in order, on the key column --
//! so "the same rows, read as one file and as six" is checked and not assumed.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::record_batch::RecordBatch;
use columnar_core::ScalarValue;
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::ParquetSource;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

/// The rows in each of the six files: one row group each.
const ROW_GROUP_ROWS: usize = 1_000_000;

/// The batch size the rows arrive in.
const BATCH_ROWS: usize = 65_536;

/// The one column the file writes plain: it is the high-cardinality one.
const COMMENT_COLUMN: &str = "l_comment";

/// The two columns the read asks for: the join key and the revenue column.
const READ_COLUMNS: [usize; 2] = [0, 5];

/// The column the rows are compared on.
const CHECK_COLUMN: &str = "l_orderkey";

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The buffer the writer's output goes through.
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;

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

/// The writer's properties: the published file's own.
fn writer_properties() -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_bloom_filter_enabled(false)
        .set_column_dictionary_enabled(ColumnPath::from(COMMENT_COLUMN), false)
        .build()
}

/// The file's rows, read once, in the writer's grid.
fn read_source(path: &Path) -> Vec<RecordBatch> {
    let file = File::open(path).expect("the source opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let reader = builder.with_batch_size(BATCH_ROWS).build().expect("the reader");
    reader.map(|batch| batch.expect("the batch")).collect()
}

/// Writes the rows as one file per row group, and returns the files.
///
/// The rows are the published file's own rows, in the published file's own
/// batches, so **the only thing that changes is the number of files**.
fn write_files(batches: &[RecordBatch], dir: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("the directory is created");
    let properties = writer_properties();
    let mut files = Vec::new();
    let mut index = 0usize;
    let mut rows_in_file = 0usize;
    let mut writer: Option<ArrowWriter<BufWriter<File>>> = None;

    for batch in batches {
        let mut offset = 0usize;
        loop {
            if writer.is_none() {
                let path = dir.join(format!("{index:06}.parquet"));
                let file = File::create(&path).expect("the file is created");
                let buffered = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
                writer = Some(
                    ArrowWriter::try_new(buffered, batch.schema(), Some(properties.clone()))
                        .expect("the writer is built"),
                );
                files.push(path);
                index += 1;
                rows_in_file = 0;
            }
            let room = ROW_GROUP_ROWS - rows_in_file;
            let take = room.min(batch.num_rows() - offset);
            let piece = if offset == 0 && take == batch.num_rows() {
                batch.clone()
            } else {
                batch.slice(offset, take)
            };
            let written = writer.as_mut().expect("the writer is there").write(&piece);
            written.expect("the write");
            rows_in_file += take;
            offset += take;
            if rows_in_file == ROW_GROUP_ROWS {
                let finished = writer.take().expect("the writer is there");
                finished.close().expect("the writer closes");
                writer = None;
            }
            if offset == batch.num_rows() {
                break;
            }
        }
    }
    if let Some(finished) = writer.take() {
        finished.close().expect("the writer closes");
    }
    files
}

/// The engine's read of one file, through the engine's own reader.
fn engine_read(path: &Path) -> (Duration, usize) {
    let started = Instant::now();
    let source = ParquetSource::open(path).expect("the source opens");
    let dataset = source
        .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The engine's read of six files, one after the other, and their sum.
fn engine_read_many(paths: &[PathBuf]) -> (Duration, usize) {
    let started = Instant::now();
    let mut rows = 0usize;
    for path in paths {
        let source = ParquetSource::open(path).expect("the source opens");
        let dataset = source
            .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), 1)
            .expect("the read runs");
        rows += dataset.len();
        std::hint::black_box(&dataset);
    }
    (started.elapsed(), rows)
}

/// Whether two values are the same value.
fn same_value(left: Option<ScalarValue>, right: Option<ScalarValue>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(ScalarValue::Int64(a)), Some(ScalarValue::Int64(b))) => a == b,
        (Some(ScalarValue::Float64(a)), Some(ScalarValue::Float64(b))) => {
            a.to_bits() == b.to_bits()
        }
        (Some(ScalarValue::Utf8(a)), Some(ScalarValue::Utf8(b))) => a == b,
        _ => false,
    }
}

/// Checks that the six files' rows are the one file's rows, in the same order.
///
/// The check is on the key column, row by row: the engine's own reader, the
/// engine's own column, and the engine's own values.
fn same_rows(one: &Path, many: &[PathBuf]) -> bool {
    let source = ParquetSource::open(one).expect("the source opens");
    let dataset = source
        .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let column = dataset.chunk(CHECK_COLUMN).expect("the column is there");
    let total = dataset.len();
    let mut row = 0usize;
    for path in many {
        let source = ParquetSource::open(path).expect("the source opens");
        let part = source
            .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), 1)
            .expect("the read runs");
        let part_column = part.chunk(CHECK_COLUMN).expect("the column is there");
        for index in 0..part.len() {
            if !same_value(column.value_at(row + index), part_column.value_at(index)) {
                return false;
            }
        }
        row += part.len();
    }
    row == total
}

/// The probe's arguments.
struct Arguments {
    /// The file the probe reads and writes.
    file: PathBuf,
    /// Where the report goes.
    out: Option<String>,
    /// Where the probe's own files go.
    work_dir: PathBuf,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut arguments = std::env::args().skip(1);
    let mut file = PathBuf::from("data/lineitem.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/many-files-probe");
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
    println!("iterations: {ITERATIONS}\n");

    let batches = read_source(&path);
    println!("--- the source's own rows, read once ---");
    println!("batches: {}\n", batches.len());

    let files = write_files(&batches, &work_dir.join("files"));
    println!("--- the same rows, as {} files ---", files.len());
    for file in &files {
        println!("  {}", file.display());
    }
    println!();

    let mut one_samples = Vec::with_capacity(ITERATIONS);
    let mut many_samples = Vec::with_capacity(ITERATIONS);
    let mut rows_one = 0usize;
    let mut rows_many = 0usize;
    for _ in 0..ITERATIONS {
        let (one, one_rows) = engine_read(&path);
        let (many, many_rows) = engine_read_many(&files);
        one_samples.push(one);
        many_samples.push(many);
        rows_one = one_rows;
        rows_many = many_rows;
    }

    let identical = same_rows(&path, &files);

    let one_timing = Timing::of(&one_samples);
    let many_timing = Timing::of(&many_samples);
    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    medians.insert("read_one_file".to_owned(), serde_json::json!(one_timing.median));
    medians.insert("read_six_files".to_owned(), serde_json::json!(many_timing.median));
    spreads.insert("read_one_file".to_owned(), serde_json::json!([one_timing.min, one_timing.max]));
    spreads
        .insert("read_six_files".to_owned(), serde_json::json!([many_timing.min, many_timing.max]));

    let boundary = many_timing.median - one_timing.median;
    let share = if one_timing.median > 0.0 { boundary / one_timing.median } else { 0.0 };
    let derived = serde_json::json!({
        "boundary_ms": boundary,
        "boundary_share": share,
    });

    println!("--- the read ---");
    print_timings(&medians, &["read_one_file", "read_six_files"]);
    println!("the boundary's cost: {boundary:.3} ms, {share:.3} of the one file's read");
    println!("rows: {rows_one} and {rows_many}, the same rows: {identical}\n");

    let report = serde_json::json!({
        "probe": "many-files",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "files": files.len(),
        "rows": rows_one,
        "batch_rows": BATCH_ROWS,
        "row_group_rows": ROW_GROUP_ROWS,
        "identical": identical,
        "rows_match": rows_one == rows_many,
        "measurements_ms": medians,
        "spreads_ms": spreads,
        "derived_ms": derived,
    });

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&work_dir).ok();
}
