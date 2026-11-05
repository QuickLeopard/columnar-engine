//! The store's own write, against the file it reproduces.
//!
//! ```text
//! cargo run --release -p columnar-store --example store_probe -- \
//!     data/lineitem.parquet --out bench-results/store-write-run1.json
//! ```
//!
//! # The question
//!
//! Spec 037 measured the write side and found that **the writer's output is a
//! function of the batch size the rows arrive in**: the Parquet writer splits
//! every batch it is handed into mini-batches of 1,024 values, and the
//! dictionary's fallback to plain is decided at one of those boundaries. The
//! store took that as a rule -- the grid is 65,536 rows, and the store checks
//! it -- and this probe says whether the rule holds.
//!
//! **It writes the published file's own rows through the store, and compares
//! the file the store wrote with the published file, byte for byte.** So the
//! store's claim is not "it wrote a file" but "**it wrote the file the
//! generator wrote, and the two are equal**".
//!
//! # What the probe measures
//!
//! | | |
//! |---|---|
//! | **`write_file`** | the writer's own write: the rows in, a Parquet file out |
//! | **`commit`** | the five steps: the data file durable, the manifest written, made durable and renamed over the old one, and the directory durable |
//! | **`append`** | the store's own API, end to end: the rows in, the commit, the manifest on disk |
//!
//! # The policy is the caller's
//!
//! The store does not decide the encoding; the **caller** does. This probe
//! passes the **published file's own** policy -- the dictionary on, and the
//! high-cardinality comment column plain -- because that is the policy the
//! comparison is against. **A store that wrote the same rows under a different
//! policy would write a different file, and that would be the caller's
//! choice, not a difference in the store.**

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::record_batch::RecordBatch;
use columnar_store::commit::commit;
use columnar_store::manifest::Manifest;
use columnar_store::writer::{Writer, BATCH_ROWS, ROW_GROUP_ROWS};
use columnar_store::{writer_properties, Store};
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

/// The file's rows, read once, in the store's grid.
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
    let mut work_dir = PathBuf::from("target/store-probe");
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

/// One pass of the store's own path: the write, the commit, and the file.
struct Pass {
    /// The writer's own write.
    write: Duration,
    /// The commit's five steps.
    commit: Duration,
    /// The file the store wrote.
    path: PathBuf,
    /// Its rows.
    rows: u64,
    /// Its bytes.
    bytes: u64,
}

/// The store's own path, once: the rows in, a file out, and the commit.
fn store_pass(dir: &Path, batches: &[RecordBatch]) -> Pass {
    let table_dir = dir.join("lineitem");
    std::fs::create_dir_all(&table_dir).expect("the table's directory is created");

    let schema = batches[0].schema();
    let write_started = Instant::now();
    let mut writer = Writer::create_with(
        &table_dir,
        "000001.parquet",
        schema,
        writer_properties(&[COMMENT_COLUMN]),
    )
    .expect("the writer is built");
    for batch in batches {
        writer.write(batch).expect("the write");
    }
    let entry = writer.close().expect("the writer closes");
    let write = write_started.elapsed();

    let mut manifest = Manifest::new("lineitem");
    manifest.add(entry);
    let commit_started = Instant::now();
    commit(&table_dir, &manifest).expect("the commit");
    let committed = commit_started.elapsed();

    Pass {
        write,
        commit: committed,
        path: table_dir.join("000001.parquet"),
        rows: manifest.rows,
        bytes: manifest.bytes,
    }
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
    println!("grid: {BATCH_ROWS} rows, row groups of {ROW_GROUP_ROWS} rows");
    println!("iterations: {ITERATIONS}\n");

    std::fs::create_dir_all(&work_dir).expect("the work directory is created");
    let batches = read_source(&path);
    println!("--- the source's own rows, read once ---");
    println!("batches: {}\n", batches.len());

    let mut writes = Vec::with_capacity(ITERATIONS);
    let mut commits = Vec::with_capacity(ITERATIONS);
    let mut identical = true;
    let mut first_difference = None;
    let mut rows = 0u64;
    let mut bytes = 0u64;

    for _ in 0..ITERATIONS {
        let pass = store_pass(&work_dir, &batches);
        writes.push(pass.write);
        commits.push(pass.commit);
        rows = pass.rows;
        bytes = pass.bytes;
        let (same, difference) = compare_files(&path, &pass.path);
        identical = identical && same;
        if first_difference.is_none() {
            first_difference = difference;
        }
    }

    // And the store's own API, end to end: the rows in, the commit, and the
    // manifest on disk.
    let store_dir = work_dir.join("store");
    let store = Store::open(&store_dir).expect("the store opens");
    let append_started = Instant::now();
    let manifest = store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let append = append_started.elapsed();
    let (api_same, api_difference) = compare_files(
        &path,
        &store.file_path("lineitem", &manifest.files.first().expect("a file").name),
    );
    identical = identical && api_same;
    if first_difference.is_none() {
        first_difference = api_difference;
    }

    let write_timing = Timing::of(&writes);
    let commit_timing = Timing::of(&commits);
    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    medians.insert("write_file".to_owned(), serde_json::json!(write_timing.median));
    medians.insert("commit".to_owned(), serde_json::json!(commit_timing.median));
    medians.insert("append".to_owned(), serde_json::json!(ms(append)));
    spreads
        .insert("write_file".to_owned(), serde_json::json!([write_timing.min, write_timing.max]));
    spreads.insert("commit".to_owned(), serde_json::json!([commit_timing.min, commit_timing.max]));
    spreads.insert("append".to_owned(), serde_json::json!([ms(append), ms(append)]));

    println!("--- the store's own write ---");
    print_timings(&medians, &["write_file", "commit", "append"]);
    println!();
    println!("--- the bytes ---");
    println!("original: {} bytes", file_bytes(&path));
    println!("written:  {bytes} bytes");
    println!(
        "identical: {}{}",
        identical,
        first_difference
            .map_or_else(String::new, |offset| format!(", first difference at {offset}"))
    );
    println!();

    let report = serde_json::json!({
        "probe": "store-write",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "rows": rows,
        "batch_rows": BATCH_ROWS,
        "row_group_rows": ROW_GROUP_ROWS,
        "bytes_original": file_bytes(&path),
        "bytes_written": bytes,
        "identical": identical,
        "first_difference": first_difference,
        "measurements_ms": medians,
        "spreads_ms": spreads,
    });

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&work_dir).ok();
}
