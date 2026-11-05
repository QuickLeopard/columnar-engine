//! The source over a set of files: what it costs, and whether it is right.
//!
//! ```text
//! cargo run --release -p columnar-engine --example file_set_probe -- \
//!     data/lineitem.parquet --out bench-results/file-set-run1.json
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
//! **And the boundary is free.** Spec 039 measured it: the published file's own six row groups, read as
//! one file and as six, came to 66.003 ms and 64.114 ms, and the difference is -1.152 ms -- negative, and
//! inside the one file's own spread. **So the source's case is correctness, not performance**, and it
//! has to be **correct and free**: a source that read the set correctly but slower than reading the files
//! one by one would be a worse answer than the one the measurement rules out.
//!
//! # The experiment
//!
//! **The published file's six row groups are the same rows as the published file.** So they are the
//! natural experiment: the probe writes them as **six files**, and then reads
//!
//! - **the one file** -- the published file, through the engine's own reader;
//! - **the six files, one by one** -- each through the engine's own reader, and the sum. This is the
//!   naive way, and it is the answer the source has to beat;
//! - **the six files, as one table** -- through the engine's new source over a set of files.
//!
//! **The only difference between the three reads is the number of files, and how they are read.** The
//! rows are the same, the columns are the same, and the host is the same.
//!
//! # The answers are checked
//!
//! The set's rows are compared with the one file's, **row by row**, in order, on the key column -- so
//! "the same rows, read as one file and as a set" is checked and not assumed. **And the set's own query
//! is checked against the one file's query**: the same plan, over the file and over the set, and the
//! counts have to agree.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Predicate, ScalarValue};
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::{
    execute_on, Aggregate, AggregateSpec, AggregateValue, ExecOptions, FileSetSource,
    ParquetSource, QueryPlan,
};
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

/// The column the query filters on.
const FILTER_COLUMN: &str = "l_quantity";

/// The threshold the query filters on.
const FILTER_VALUE: f64 = 24.0;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The threads the parallel read uses.
const THREADS: usize = 4;

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
    output.map_or_else(
        || 0.0,
        |output| {
            String::from_utf8_lossy(&output.stdout)
                .into_owned()
                .replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
                .unwrap_or(0.0)
        },
    )
}

/// The writer's properties: the published file's own.
fn writer_properties() -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_max_row_group_size(ROW_GROUP_ROWS)
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
///
/// **This is the naive way**, and it is the answer the source has to beat:
/// the rows of a filtered table are split across the files, so this does not
/// answer the query.
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

/// The engine's read of six files, as one table, through the engine's own
/// source over a set of files.
fn engine_read_set(paths: &[PathBuf]) -> (Duration, usize) {
    let started = Instant::now();
    let set = FileSetSource::open(paths).expect("the set opens");
    let dataset = set
        .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The engine's parallel read of one file, through the engine's own reader.
fn engine_read_parallel(path: &Path) -> (Duration, usize) {
    let started = Instant::now();
    let source = ParquetSource::open(path).expect("the source opens");
    let dataset = source
        .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), THREADS)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The engine's parallel read of six files, as one table.
fn engine_read_set_parallel(paths: &[PathBuf]) -> (Duration, usize) {
    let started = Instant::now();
    let set = FileSetSource::open(paths).expect("the set opens");
    let dataset = set
        .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), THREADS)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The engine's open of one file: the footer, and nothing else.
fn engine_open(path: &Path) -> Duration {
    let started = Instant::now();
    let source = ParquetSource::open(path).expect("the source opens");
    let elapsed = started.elapsed();
    std::hint::black_box(&source);
    elapsed
}

/// The engine's open of six files: the footers, and nothing else.
fn engine_open_set(paths: &[PathBuf]) -> Duration {
    let started = Instant::now();
    let set = FileSetSource::open(paths).expect("the set opens");
    let elapsed = started.elapsed();
    std::hint::black_box(&set);
    elapsed
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

/// Checks that the set's rows are the one file's rows, in the same order.
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

    let set = FileSetSource::open(many).expect("the set opens");
    let part = set
        .read(&READ_COLUMNS, &PushdownPlan::new(), columnar_kernels::best(), 1)
        .expect("the read runs");
    let part_column = part.chunk(CHECK_COLUMN).expect("the column is there");
    if part.len() != total {
        return false;
    }
    for index in 0..part.len() {
        if !same_value(column.value_at(index), part_column.value_at(index)) {
            return false;
        }
    }
    true
}

/// The count the engine computed, as a row count.
fn counted(result: &columnar_engine::AggregateResult) -> usize {
    match result.groups.first().and_then(|group| group.values.first()) {
        Some(AggregateValue::Int(value)) => {
            usize::try_from(*value).expect("the count is not negative")
        }
        other => panic!("the count is not an integer: {other:?}"),
    }
}

/// Runs the same filtered plan over the one file and over the set, and
/// compares the counts.
///
/// **This is the correctness the naive way does not have**: the rows of a
/// filtered table are split across the files, so the naive read answers a
/// different question.
fn same_count(one: &Path, many: &[PathBuf]) -> (usize, usize) {
    let plan = QueryPlan::new(
        vec![Predicate::new(FILTER_COLUMN, ComparisonOp::Gt, ScalarValue::Float64(FILTER_VALUE))],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    );

    let source = ParquetSource::open(one).expect("the source opens");
    let from_file =
        execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs over the file");

    let set = FileSetSource::open(many).expect("the set opens");
    let from_set =
        set.execute(&plan, &ExecOptions::default()).expect("the query runs over the set");

    (counted(&from_file), counted(&from_set))
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
    let mut work_dir = PathBuf::from("target/file-set-probe");
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

/// The figures derived from the medians.
///
/// The source's cost is what the set takes **over** the one file's read,
/// which is the ideal -- the same rows, in one file. The naive gap is what
/// the set takes over the files read one by one, which is the answer it
/// replaces, and which does not answer the query at all: the rows of a
/// filtered table are split across the files. The boundary is spec 039's
/// figure, and the open gap is what the set's metadata costs over one file's.
fn derived(measured: &Measurements) -> serde_json::Value {
    let source_cost = measured.set.median - measured.one.median;
    let source_cost_share =
        if measured.one.median > 0.0 { source_cost / measured.one.median } else { 0.0 };
    let source_cost_parallel = measured.set_parallel.median - measured.one_parallel.median;
    let source_cost_parallel_share = if measured.one_parallel.median > 0.0 {
        source_cost_parallel / measured.one_parallel.median
    } else {
        0.0
    };
    serde_json::json!({
        "source_cost_ms": source_cost,
        "source_cost_share": source_cost_share,
        "source_cost_parallel_ms": source_cost_parallel,
        "source_cost_parallel_share": source_cost_parallel_share,
        "naive_gap_ms": measured.set.median - measured.many.median,
        "boundary_ms": measured.many.median - measured.one.median,
        "open_gap_ms": measured.open_set.median - measured.open_one.median,
    })
}

/// A number out of a derived figure.
fn number(derived: &serde_json::Value, key: &str) -> f64 {
    derived.get(key).and_then(serde_json::Value::as_f64).unwrap_or(0.0)
}

/// The five measurements the probe takes, and the rows they read.
struct Measurements {
    /// The one file's read.
    one: Timing,
    /// The six files, read one by one.
    many: Timing,
    /// The six files, read as one table.
    set: Timing,
    /// The one file's parallel read.
    one_parallel: Timing,
    /// The six files, read as one table, in parallel.
    set_parallel: Timing,
    /// The rows the one file's read returned.
    rows_one: usize,
    /// The rows the six files' read returned.
    rows_many: usize,
    /// The rows the set's read returned.
    rows_set: usize,
    /// The one file's open: the footer, and nothing else.
    open_one: Timing,
    /// The six files' open: the footers, and nothing else.
    open_set: Timing,
}

/// Takes the five measurements, in one campaign and on the same host.
fn measure(path: &Path, files: &[PathBuf]) -> Measurements {
    let mut one_samples = Vec::with_capacity(ITERATIONS);
    let mut many_samples = Vec::with_capacity(ITERATIONS);
    let mut set_samples = Vec::with_capacity(ITERATIONS);
    let mut one_parallel_samples = Vec::with_capacity(ITERATIONS);
    let mut set_parallel_samples = Vec::with_capacity(ITERATIONS);
    let mut rows_one = 0usize;
    let mut rows_many = 0usize;
    let mut rows_set = 0usize;
    for _ in 0..ITERATIONS {
        let (one, one_rows) = engine_read(path);
        let (many, many_rows) = engine_read_many(files);
        let (set, set_rows) = engine_read_set(files);
        let (one_parallel, _) = engine_read_parallel(path);
        let (set_parallel, _) = engine_read_set_parallel(files);
        one_samples.push(one);
        many_samples.push(many);
        set_samples.push(set);
        one_parallel_samples.push(one_parallel);
        set_parallel_samples.push(set_parallel);
        rows_one = one_rows;
        rows_many = many_rows;
        rows_set = set_rows;
    }
    let mut open_one_samples = Vec::with_capacity(ITERATIONS);
    let mut open_set_samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        open_one_samples.push(engine_open(path));
        open_set_samples.push(engine_open_set(files));
    }
    Measurements {
        open_one: Timing::of(&open_one_samples),
        open_set: Timing::of(&open_set_samples),
        one: Timing::of(&one_samples),
        many: Timing::of(&many_samples),
        set: Timing::of(&set_samples),
        one_parallel: Timing::of(&one_parallel_samples),
        set_parallel: Timing::of(&set_parallel_samples),
        rows_one,
        rows_many,
        rows_set,
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

    let measured = measure(&path, &files);
    let identical = same_rows(&path, &files);
    let (count_one, count_set) = same_count(&path, &files);

    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    for (name, timing) in [
        ("read_one_file", measured.one),
        ("read_six_files", measured.many),
        ("read_file_set", measured.set),
        ("read_one_file_parallel", measured.one_parallel),
        ("read_file_set_parallel", measured.set_parallel),
        ("open_one_file", measured.open_one),
        ("open_file_set", measured.open_set),
    ] {
        medians.insert(name.to_owned(), serde_json::json!(timing.median));
        spreads.insert(name.to_owned(), serde_json::json!([timing.min, timing.max]));
    }

    let derived = derived(&measured);
    let source_cost = number(&derived, "source_cost_ms");
    let source_cost_share = number(&derived, "source_cost_share");
    let source_cost_parallel = number(&derived, "source_cost_parallel_ms");
    let source_cost_parallel_share = number(&derived, "source_cost_parallel_share");
    let naive_gap = number(&derived, "naive_gap_ms");
    let open_gap = number(&derived, "open_gap_ms");

    println!("--- the read ---");
    print_timings(
        &medians,
        &[
            "read_one_file",
            "read_six_files",
            "read_file_set",
            "read_one_file_parallel",
            "read_file_set_parallel",
            "open_one_file",
            "open_file_set",
        ],
    );
    println!(
        "the source's cost: {source_cost:.3} ms, {source_cost_share:.3} of the one file's read"
    );
    println!(
        "the source's cost, parallel: {source_cost_parallel:.3} ms, {source_cost_parallel_share:.3} of the parallel one file's read"
    );
    println!("the set over the naive way: {naive_gap:.3} ms");
    println!("the open: {open_gap:.3} ms over the one file's");
    println!("rows: {}, {} and {}", measured.rows_one, measured.rows_many, measured.rows_set);
    println!("the same rows: {identical}");
    println!("the filtered count: {count_one} over the file, {count_set} over the set\n");

    let report = serde_json::json!({
        "probe": "file-set",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "files": files.len(),
        "rows": measured.rows_one,
        "batch_rows": BATCH_ROWS,
        "row_group_rows": ROW_GROUP_ROWS,
        "identical": identical,
        "rows_match": measured.rows_one == measured.rows_many
            && measured.rows_one == measured.rows_set,
        "count_one_file": count_one,
        "count_file_set": count_set,
        "count_match": count_one == count_set,
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
