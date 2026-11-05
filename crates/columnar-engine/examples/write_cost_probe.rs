//! The write side, taken apart.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement behind
//! spec 037's question: **what does the write side cost?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example write_cost_probe -- \
//!     data/lineitem.parquet --out bench-results/write-cost-run1.json
//! ```
//!
//! # The question
//!
//! Every published figure in this repository is a figure about **reading**. Thirty-four
//! specs of them: the scan's decode, the join's probe, the sort's heap, the drift's own
//! parts. **Nothing in the tree writes a table.** The one writer is `columnar-testdata`'s
//! dataset generator -- a benchmark fixture, `publish = false`, with a fixed TPC-H schema
//! and a scale factor as its only parameter. It is not a write path, and it was never
//! measured as one.
//!
//! An embedded store is made of three things the engine does not have: a **write path**,
//! a **commit protocol**, and a **catalog that survives the process**. None of the three
//! has a measurement, and this repository measures before it builds. So the probe prices
//! them, and three numbers decide the store's shape:
//!
//! - how long a row group takes to write, against the read's own cost;
//! - what a commit costs, and above all what an `fsync` costs on this host;
//! - what the write does to the file's bytes, and therefore to the read that follows.
//!
//! # What the probe does
//!
//! It reads the published file's **own rows** once, and writes them back out through the
//! Parquet writer, in the file's **own row groups** of a million rows, with the file's
//! **own writer properties**. Then it commits the file the way an append-only store
//! commits it, and it reads the result back through the **engine's own reader**.
//!
//! | | |
//! |---|---|
//! | **`write_row_group`** | one row group -- a million rows -- written out |
//! | **`write_file`** | the whole file, six row groups, written out |
//! | **`write_plain`** | the whole file again, with the dictionary off |
//! | **`write_manifest`** | writing the manifest that names the file |
//! | **`fsync_data`** | the `fsync` of the data file |
//! | **`fsync_manifest`** | the `fsync` of the manifest |
//! | **`rename_manifest`** | the `rename` of the manifest over the old one |
//! | **`fsync_dir`** | the `fsync` of the directory |
//! | **`commit`** | all five steps, as one sequence |
//! | **`read_scan_original`** | the engine's read of the original, two columns |
//! | **`read_scan_written`** | the engine's read of the file the probe wrote |
//! | **`read_scan_plain`** | the engine's read of the file written plain |
//! | **`read_all_original`** | the engine's read of the original, all sixteen columns |
//! | **`read_all_written`** | the engine's read of the file the probe wrote |
//! | **`read_all_plain`** | the engine's read of the file written plain |
//!
//! # The write is measured without the read that fed it
//!
//! The probe reads the file **once**, before the clock starts, and holds the rows in
//! memory. Every write is then timed on its own. So the write's figure is the writer's
//! own cost, and the read that fed it is not in it -- the store's caller brings the rows,
//! and the caller pays for bringing them.
//!
//! # The commit is the protocol's own order
//!
//! The steps are in the order a commit has to take them: the **data** file is made
//! durable first, then the **manifest** that names it is written, made durable, and
//! `rename`d over the old one, and finally the **directory** is made durable so the
//! rename itself survives. A reader that sees the new manifest sees the new data; a
//! reader that sees the old one sees the old data; there is no third answer.
//!
//! # The bytes are compared, not assumed
//!
//! The probe compares the file it wrote with the file it read **byte for byte**, and the
//! report carries the verdict and, if the two differ, the offset of the first byte that
//! differs. So "the store's writer reproduces the file" is a claim the report either
//! carries or does not.
//!
//! ## And the batch size is what decides it
//!
//! The Parquet writer splits **every batch it is handed** into mini-batches of
//! `DEFAULT_WRITE_BATCH_SIZE` -- **1,024** values -- and the dictionary's fallback to
//! plain, the point where the dictionary page crosses its one-megabyte limit, is decided
//! at one of those boundaries. A batch size that is a multiple of 1,024 puts the
//! boundaries on the same value indices every time; a batch size that is not makes them
//! drift.
//!
//! So the file the probe writes is a function of **the batch size the rows arrive in**,
//! and the probe takes that size as an argument for exactly this reason. The published
//! file was written in batches of **65,536** -- 64 x 1,024 -- and at that batch size the
//! probe reproduces it **byte for byte**. At the engine's own derived batch size --
//! **125,000**, which is 122 x 1,024 + 192 -- the boundaries drift, the dictionary's
//! fallback lands later, and the file comes out **different**. The difference is in the
//! dictionary page: it holds **131,685** values instead of **131,616**.
//!
//! # What the figures are not
//!
//! These are **cold** figures on a **shared** host, taken five times each and reported as
//! the median with the spread beside it. The report carries the host's load average, so a
//! figure that drifted can be read against it.
//!
//! **And the `fsync` measured here is the platform's `fsync`, not a drive's flush.** On
//! APFS the two are different: `fsync` makes the file's data durable with respect to the
//! operating system, while `F_FULLFSYNC` additionally flushes the drive's write cache.
//! Rust's `File::sync_all` is the former. The number below is therefore the **logical**
//! flush's cost, and the durability it buys on this platform is weaker than the number
//! suggests. The store's commit protocol is a decision that has to be checked on the
//! platform it will ship on.

#![allow(clippy::indexing_slicing, clippy::cast_precision_loss)]

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::array::RecordBatch;
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::{ParquetSource, MAX_READER_BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::schema::types::ColumnPath;

/// The rows in a row group of the published file.
const ROW_GROUP_ROWS: usize = 1_000_000;

/// The batch size the published file was written in.
///
/// The Parquet writer splits every batch it is handed into mini-batches of
/// `DEFAULT_WRITE_BATCH_SIZE` -- 1,024 values -- and the dictionary's fallback to
/// plain is decided at one of those boundaries. A batch size that is a multiple of
/// 1,024 puts the boundaries on the same value indices every time; a batch size that
/// is not makes them drift, and the file that comes out is a different file.
const GENERATOR_BATCH_ROWS: usize = 65_536;

/// The one column the file writes plain: it is the high-cardinality one.
const COMMENT_COLUMN: &str = "l_comment";

/// The two columns the lazy scan reads: the join key and the revenue column.
const SCAN_COLUMNS: [usize; 2] = [0, 5];

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The buffer the writer's output goes through.
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;

/// The chunk the byte comparison reads in.
const COMPARE_CHUNK_BYTES: usize = 64 * 1024;

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
/// The first sample pays for the page cache, and it is kept: the median of five is the
/// figure, and the spread beside it says how much the host moved while it was taken.
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
/// The host prints it as `{ 5.85 5.27 4.63 }`, so the braces come off before the
/// first number is read.
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

/// The file's writer properties, with the dictionary on or off.
///
/// These are the **published file's own** properties -- SNAPPY, the dictionary on, the
/// high-cardinality comment column plain, chunk-level statistics, Parquet 1.0 -- so that
/// the file the probe writes is the file the generator wrote, and the byte comparison
/// says whether it is.
fn writer_properties(row_group_rows: usize, dictionary: bool) -> WriterProperties {
    let mut builder = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(dictionary)
        .set_max_row_group_size(row_group_rows)
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_bloom_filter_enabled(false);
    if dictionary {
        builder = builder.set_column_dictionary_enabled(ColumnPath::from(COMMENT_COLUMN), false);
    }
    builder.build()
}

/// The file's rows, read once, as Arrow batches.
///
/// The read is **outside** every write's clock: the store's caller brings the rows, and
/// the caller pays for bringing them.
fn read_source(path: &Path, batch_rows: usize) -> Vec<RecordBatch> {
    let file = File::open(path).expect("the source opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let reader = builder.with_batch_size(batch_rows).build().expect("the reader builds");
    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch.expect("the batch"));
    }
    batches
}

/// Writes rows out, and returns how long it took.
///
/// The clock covers the file's creation, the writer's construction, the rows and the
/// writer's close -- which is where the file's footer is written.
fn write_rows(
    batches: &[RecordBatch],
    path: &Path,
    properties: WriterProperties,
    row_limit: Option<usize>,
) -> Duration {
    let started = Instant::now();
    let file = File::create(path).expect("the file is created");
    let buffered = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
    let schema = batches[0].schema();
    let mut writer =
        ArrowWriter::try_new(buffered, schema, Some(properties)).expect("the writer is built");
    let mut written = 0usize;
    for batch in batches {
        if let Some(limit) = row_limit {
            if written >= limit {
                break;
            }
            let take = limit - written;
            if take < batch.num_rows() {
                writer.write(&batch.slice(0, take)).expect("the write");
                break;
            }
        }
        writer.write(batch).expect("the write");
        written += batch.num_rows();
    }
    writer.close().expect("the writer closes");
    started.elapsed()
}

/// The `fsync` of a file.
fn fsync_file(path: &Path) -> Duration {
    let file = File::open(path).expect("the file opens");
    let started = Instant::now();
    file.sync_all().expect("the file syncs");
    started.elapsed()
}

/// The `fsync` of a directory, which is what makes a `rename` survive.
fn fsync_dir(path: &Path) -> Duration {
    let dir = File::open(path).expect("the directory opens");
    let started = Instant::now();
    dir.sync_all().expect("the directory syncs");
    started.elapsed()
}

/// The commit's five steps, each measured on its own.
#[derive(Debug, Clone, Copy)]
struct Commit {
    /// Writing the manifest.
    write_manifest: Duration,
    /// The `fsync` of the data file.
    fsync_data: Duration,
    /// The `fsync` of the manifest.
    fsync_manifest: Duration,
    /// The `rename` of the manifest over the old one.
    rename_manifest: Duration,
    /// The `fsync` of the directory.
    fsync_dir: Duration,
    /// The whole sequence, in the order a commit has to take it.
    total: Duration,
}

/// The manifest that names the committed file.
fn manifest_body(data: &Path, rows: u64, bytes: u64) -> String {
    serde_json::json!({
        "format": "columnar-store/1",
        "table": "lineitem",
        "file": data.file_name().unwrap_or_default().to_string_lossy(),
        "rows": rows,
        "bytes": bytes,
        "row_groups": 1,
    })
    .to_string()
}

/// The commit, in the order a commit has to take it.
///
/// The data file is made durable first, then the manifest that names it is written, made
/// durable, and `rename`d over the old one, and the directory is made durable last so
/// that the rename itself survives.
fn commit_file(data: &Path, dir: &Path, rows: u64, bytes: u64) -> Commit {
    let manifest_tmp = dir.join("manifest.json.tmp");
    let manifest = dir.join("manifest.json");
    let body = manifest_body(data, rows, bytes);

    let started = Instant::now();

    let fsync_data = fsync_file(data);

    let write_started = Instant::now();
    let mut file = File::create(&manifest_tmp).expect("the manifest is created");
    file.write_all(body.as_bytes()).expect("the manifest is written");
    let write_manifest = write_started.elapsed();

    let fsync_manifest = fsync_file(&manifest_tmp);

    let rename_started = Instant::now();
    std::fs::rename(&manifest_tmp, &manifest).expect("the rename");
    let rename_manifest = rename_started.elapsed();

    let fsync_dir = fsync_dir(dir);

    Commit {
        write_manifest,
        fsync_data,
        fsync_manifest,
        rename_manifest,
        fsync_dir,
        total: started.elapsed(),
    }
}

/// The engine's own read of a file, through the engine's own reader.
fn engine_read(path: &Path, columns: &[usize]) -> Duration {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source
        .read(columns, &PushdownPlan::new(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let elapsed = started.elapsed();
    std::hint::black_box(&dataset);
    elapsed
}

/// Compares two files byte for byte, and says where they first differ.
fn compare_files(left: &Path, right: &Path) -> (bool, Option<u64>) {
    let mut left_reader = BufReader::new(File::open(left).expect("the left file opens"));
    let mut right_reader = BufReader::new(File::open(right).expect("the right file opens"));
    let mut left_chunk = vec![0u8; COMPARE_CHUNK_BYTES];
    let mut right_chunk = vec![0u8; COMPARE_CHUNK_BYTES];
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

/// The file's row-group count.
fn row_groups(path: &Path) -> usize {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    reader.metadata().num_row_groups()
}

/// The file's row count.
fn rows(path: &Path) -> u64 {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    u64::try_from(reader.metadata().file_metadata().num_rows()).unwrap_or(0)
}

/// The batch size the engine's own read uses on a file.
///
/// The engine derives it from the file: the greatest common divisor of the row groups'
/// row counts, rounded down to a divisor that fits under the limit. The probe asks the
/// engine for it, so that the probe's shapes are the engine's shapes by construction
/// rather than by a copy of the rule.
fn batch_rows(path: &Path) -> usize {
    let source = ParquetSource::open(path).expect("the source opens");
    source.batch_rows_under(MAX_READER_BATCH_ROWS)
}

/// A file's size in bytes.
fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// Prints a set of measurements, with their spreads.
fn print_timings(measurements: &Measurements, names: &[&str], width: usize, places: usize) {
    for name in names {
        let median = measurements.medians.get(*name).and_then(serde_json::Value::as_f64);
        let spread = measurements.spreads.get(*name).and_then(serde_json::Value::as_array);
        let min = spread.and_then(|pair| pair.first()).and_then(serde_json::Value::as_f64);
        let max = spread.and_then(|pair| pair.get(1)).and_then(serde_json::Value::as_f64);
        println!(
            "{name:<width$}: {:>8.places$} ms  [{:>7.places$} .. {:>7.places$}]",
            median.unwrap_or(0.0),
            min.unwrap_or(0.0),
            max.unwrap_or(0.0)
        );
    }
}

/// The probe's arguments.
struct Arguments {
    /// The file the probe reads and writes.
    file: PathBuf,
    /// Where the report goes.
    out: Option<String>,
    /// Where the probe's own files go.
    work_dir: PathBuf,
    /// The batch size the rows arrive in, when the caller names one.
    batch_rows: Option<usize>,
    /// Whether the probe's own files stay behind.
    keep: bool,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut arguments = std::env::args().skip(1);
    let mut file = PathBuf::from("data/lineitem.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/write-cost-probe");
    let mut batch_rows = None;
    let mut keep = false;
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
            "--keep" => keep = true,
            "--batch-rows" => {
                if let Some(value) = arguments.next() {
                    batch_rows = value.parse::<usize>().ok();
                }
            }
            _ => {}
        }
    }
    Arguments { file, out, work_dir, batch_rows, keep }
}

/// The measurements, and the report they make.
struct Measurements {
    /// Each measurement's median, in milliseconds.
    medians: serde_json::Map<String, serde_json::Value>,
    /// Each measurement's spread, in milliseconds.
    spreads: serde_json::Map<String, serde_json::Value>,
}

impl Measurements {
    /// A measurement, recorded.
    fn record(&mut self, name: &str, timing: Timing) {
        self.medians.insert(name.to_owned(), serde_json::json!(timing.median));
        self.spreads.insert(name.to_owned(), serde_json::json!([timing.min, timing.max]));
    }
}

/// The write shapes: the whole file, one row group, and the file written plain.
fn write_shapes(
    measurements: &mut Measurements,
    batches: &[RecordBatch],
    written: &Path,
    plain: &Path,
    properties: &WriterProperties,
) {
    let row_group =
        repeat(|| write_rows(batches, written, properties.clone(), Some(ROW_GROUP_ROWS)));
    measurements.record("write_row_group", row_group);

    let whole = repeat(|| write_rows(batches, written, properties.clone(), None));
    measurements.record("write_file", whole);

    let plain_properties = writer_properties(ROW_GROUP_ROWS, false);
    let plain_write = repeat(|| write_rows(batches, plain, plain_properties.clone(), None));
    measurements.record("write_plain", plain_write);
}

/// The commit shapes: the five steps, and the whole sequence.
///
/// Each step is measured on its own and reported as its own median, so that the parts
/// and the whole are the same kind of figure.
fn commit_shapes(measurements: &mut Measurements, data: &Path, dir: &Path, rows: u64, bytes: u64) {
    let mut writes = Vec::with_capacity(ITERATIONS);
    let mut data_syncs = Vec::with_capacity(ITERATIONS);
    let mut manifest_syncs = Vec::with_capacity(ITERATIONS);
    let mut renames = Vec::with_capacity(ITERATIONS);
    let mut dir_syncs = Vec::with_capacity(ITERATIONS);
    let mut totals = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let commit = commit_file(data, dir, rows, bytes);
        writes.push(commit.write_manifest);
        data_syncs.push(commit.fsync_data);
        manifest_syncs.push(commit.fsync_manifest);
        renames.push(commit.rename_manifest);
        dir_syncs.push(commit.fsync_dir);
        totals.push(commit.total);
    }
    measurements.record("write_manifest", Timing::of(&writes));
    measurements.record("fsync_data", Timing::of(&data_syncs));
    measurements.record("fsync_manifest", Timing::of(&manifest_syncs));
    measurements.record("rename_manifest", Timing::of(&renames));
    measurements.record("fsync_dir", Timing::of(&dir_syncs));
    measurements.record("commit", Timing::of(&totals));
}

/// The read-back shapes: the engine's own reader, on the three files.
fn read_shapes(measurements: &mut Measurements, original: &Path, written: &Path, plain: &Path) {
    let all: Vec<usize> = (0..16).collect();
    for (name, path) in [
        ("read_scan_original", original),
        ("read_scan_written", written),
        ("read_scan_plain", plain),
    ] {
        let timing = repeat(|| engine_read(path, &SCAN_COLUMNS));
        measurements.record(name, timing);
    }
    for (name, path) in
        [("read_all_original", original), ("read_all_written", written), ("read_all_plain", plain)]
    {
        let timing = repeat(|| engine_read(path, &all));
        measurements.record(name, timing);
    }
}

/// The figures derived from the measurements.
fn derived(measurements: &Measurements, bytes: &Bytes) -> serde_json::Value {
    let median = |name: &str| -> f64 {
        measurements.medians.get(name).and_then(serde_json::Value::as_f64).unwrap_or(0.0)
    };
    let write_file = median("write_file");
    let write_row_group = median("write_row_group");
    let groups = f64::from(u32::try_from(bytes.row_groups.saturating_sub(1).max(1)).unwrap_or(1));
    let per_row_group = (write_file - write_row_group) / groups;
    let read_all = median("read_all_original");
    let read_scan = median("read_scan_original");
    let commit = median("commit");
    let fsyncs = median("fsync_data") + median("fsync_manifest") + median("fsync_dir");
    let rows = bytes.rows as f64;
    serde_json::json!({
        "ms_per_row_group": per_row_group,
        "write_over_read_all": if read_all > 0.0 { write_file / read_all } else { 0.0 },
        "write_over_read_scan": if read_scan > 0.0 { write_file / read_scan } else { 0.0 },
        "commit_over_write": if write_file > 0.0 { commit / write_file } else { 0.0 },
        "fsyncs_over_commit": if commit > 0.0 { fsyncs / commit } else { 0.0 },
        "bytes_per_row_original": if rows > 0.0 { bytes.original as f64 / rows } else { 0.0 },
        "bytes_per_row_written": if rows > 0.0 { bytes.written as f64 / rows } else { 0.0 },
        "bytes_per_row_plain": if rows > 0.0 { bytes.plain as f64 / rows } else { 0.0 },
    })
}

/// The three files' sizes, and the verdict on their identity.
struct Bytes {
    /// The original file's size.
    original: u64,
    /// The file the probe wrote.
    written: u64,
    /// The file the probe wrote with the dictionary off.
    plain: u64,
    /// The file's row count.
    rows: u64,
    /// The file's row-group count.
    row_groups: usize,
    /// Whether the file the probe wrote is the original, byte for byte.
    identical: bool,
    /// Where the two first differ, if they do.
    first_difference: Option<u64>,
}

/// The files' sizes, and the verdict on their identity.
fn bytes_phase(path: &Path, written: &Path, plain: &Path, file_rows: u64, groups: usize) -> Bytes {
    let (identical, first_difference) = compare_files(path, written);
    let bytes = Bytes {
        original: file_bytes(path),
        written: file_bytes(written),
        plain: file_bytes(plain),
        rows: file_rows,
        row_groups: groups,
        identical,
        first_difference,
    };
    println!("--- the bytes ---");
    println!("original: {} bytes", bytes.original);
    println!("written:  {} bytes", bytes.written);
    println!("plain:    {} bytes", bytes.plain);
    println!(
        "identical: {}{}",
        bytes.identical,
        bytes
            .first_difference
            .map_or_else(String::new, |offset| format!(", first difference at {offset}"))
    );
    println!();
    bytes
}

fn main() {
    let arguments = arguments();
    let path = arguments.file.clone();
    let work_dir = arguments.work_dir.clone();

    let groups = row_groups(&path);
    let file_rows = rows(&path);
    let batch = arguments.batch_rows.unwrap_or(GENERATOR_BATCH_ROWS);

    println!("file: {}", path.display());
    println!("row groups: {groups}, rows: {file_rows}, batch rows: {batch}");
    println!("iterations: {ITERATIONS}\n");

    std::fs::create_dir_all(&work_dir).expect("the work directory is created");
    let written = work_dir.join("lineitem-written.parquet");
    let plain = work_dir.join("lineitem-plain.parquet");

    let read_started = Instant::now();
    let batches = read_source(&path, batch);
    let read_source_ms = ms(read_started.elapsed());
    println!("--- the source's own rows, read once, {read_source_ms:.2} ms ---");
    println!("batches: {}\n", batches.len());

    let mut measurements =
        Measurements { medians: serde_json::Map::new(), spreads: serde_json::Map::new() };
    let properties = writer_properties(ROW_GROUP_ROWS, true);

    println!("--- the write ---");
    write_shapes(&mut measurements, &batches, &written, &plain, &properties);
    print_timings(&measurements, &["write_row_group", "write_file", "write_plain"], 18, 2);
    println!();

    let bytes = bytes_phase(&path, &written, &plain, file_rows, groups);

    println!("--- the commit ---");
    commit_shapes(&mut measurements, &written, &work_dir, file_rows, bytes.written);
    print_timings(
        &measurements,
        &[
            "write_manifest",
            "fsync_data",
            "fsync_manifest",
            "rename_manifest",
            "fsync_dir",
            "commit",
        ],
        18,
        3,
    );
    println!();

    println!("--- the read-back ---");
    read_shapes(&mut measurements, &path, &written, &plain);
    print_timings(
        &measurements,
        &[
            "read_scan_original",
            "read_scan_written",
            "read_scan_plain",
            "read_all_original",
            "read_all_written",
            "read_all_plain",
        ],
        20,
        2,
    );
    println!();

    let derived = derived(&measurements, &bytes);
    let report = serde_json::json!({
        "probe": "write-cost",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit(),
        "host_load_average": load_average(),
        "row_groups": groups,
        "rows": file_rows,
        "row_group_rows": ROW_GROUP_ROWS,
        "batch_rows": batch,
        "engine_batch_rows": batch_rows(&path),
        "read_source_ms": read_source_ms,
        "bytes_original": bytes.original,
        "bytes_written": bytes.written,
        "bytes_plain": bytes.plain,
        "identical": bytes.identical,
        "first_difference": bytes.first_difference,
        "measurements_ms": measurements.medians,
        "spreads_ms": measurements.spreads,
        "derived_ms": derived,
    });

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    if arguments.keep {
        println!("kept: {}", work_dir.display());
    } else {
        std::fs::remove_dir_all(&work_dir).ok();
    }
}
