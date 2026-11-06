//! The join's build side, over a set of files.
//!
//! ```text
//! cargo run --release -p columnar-engine --example build_set_probe -- \
//!     data/lineitem.parquet data/orders.parquet --out bench-results/build-set-run1.json
//! ```
//!
//! # The question
//!
//! The join's build side is the right-hand table, and it is the side the hash
//! table is built from before the probe looks anything up. It was named by
//! **one path**. Spec 042 gives it **a set**, and this probe says what that
//! costs and what it does not.
//!
//! # The experiment
//!
//! The published file's own **six row groups** are written as **six files** --
//! the experiment spec 039 already used -- and the build side is built twice
//! in the same campaign: once from the **one file**, and once from the **six
//! files**. The probe side is the same in both, so the only thing that moves
//! is the build side.
//!
//! # The four questions
//!
//! | | |
//! |---|---|
//! | **the cost** | the table's construction, one file against six, and the open |
//! | **the identity** | the same rows, the same output rows, the same answer |
//! | **the shape** | the direct-addressed table's width: the key's range, and the union of the files' ranges |
//! | **the estimate** | what the budget is charged for a set, against what the build uses |
//!
//! **The estimate is what the budget check is made of**, so an estimate that
//! under-charges is a budget check that lies -- and the charge is compared
//! with the build it is supposed to cover.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::record_batch::RecordBatch;
use columnar_engine::memory::{direct_table_fits, estimate_build_side, estimate_build_side_set};
use columnar_engine::{
    execute_on, open_source, Aggregate, AggregateSpec, Expr, FileSetSource, JoinSpec,
    ParquetSource, QueryPlan, ScalarValue,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

/// The rows in a row group, and the grid the files are written on.
const ROW_GROUP_ROWS: usize = 1_000_000;

/// The rows in a batch.
const BATCH_ROWS: usize = 65_536;

/// The buffer the writer's output goes through.
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;

/// The one column the published file writes plain.
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
    let mut index = 0_usize;
    let mut rows_in_file = 0_usize;
    let mut writer: Option<ArrowWriter<BufWriter<File>>> = None;

    for batch in batches {
        let mut offset = 0_usize;
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
                finished.close().expect("the file closes");
            }
            if offset == batch.num_rows() {
                break;
            }
        }
    }
    if let Some(finished) = writer.take() {
        finished.close().expect("the last file closes");
    }
    files
}

/// The plan the probe runs, with the build side's files.
///
/// The probe side is the **orders** table, and the build side is the
/// **lineitem** table: the build side is the big one here, because the
/// question is what the *set* costs, not what a join's small side costs.
fn plan(build_paths: Vec<PathBuf>) -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("o_totalprice") }],
        }),
        join: Some(JoinSpec {
            build_paths,
            probe_key: "o_orderkey".to_owned(),
            build_key: "l_orderkey".to_owned(),
            build_filter: Vec::new(),
        }),
        sort: None,
    }
}

/// What one pass of the join produced.
struct Pass {
    /// The build table's construction.
    build: Duration,
    /// The build side's open.
    open: Duration,
    /// The join's output rows.
    rows: usize,
    /// The answer: the sum the join produced.
    answer: f64,
    /// The plan's estimate, in bytes.
    estimate: usize,
}

/// The join over a build side, once.
fn run(source: &ParquetSource, plan: &QueryPlan) -> Pass {
    let started = Instant::now();
    let result =
        execute_on(plan, source, &columnar_engine::ExecOptions::default()).expect("the join runs");
    let _ = started;
    let join = result.plan_output.join.expect("the plan had a join");
    let group = result.groups.first().expect("the group is there");
    let answer = match group.values.first().expect("the value is there") {
        columnar_engine::AggregateValue::Float(value) => *value,
        other => panic!("the sum is a float, not {other:?}"),
    };
    Pass {
        build: join.build_elapsed,
        open: join.open_elapsed,
        rows: join.output_rows,
        answer,
        estimate: result.plan_output.estimated_bytes,
    }
}

/// The probe's arguments.
struct Arguments {
    /// The file the build side is written from.
    build: PathBuf,
    /// The file the probe side is read from.
    probe: PathBuf,
    /// Where the report goes.
    out: Option<String>,
    /// Where the six files go.
    work_dir: PathBuf,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut arguments = std::env::args().skip(1);
    let mut build = PathBuf::from("data/lineitem.parquet");
    let mut probe = PathBuf::from("data/orders.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/build-set-probe");
    let mut positional = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => out = arguments.next(),
            "--work-dir" => {
                if let Some(value) = arguments.next() {
                    work_dir = PathBuf::from(value);
                }
            }
            _ => positional.push(PathBuf::from(argument)),
        }
    }
    if let Some(first) = positional.first() {
        build.clone_from(first);
    }
    if let Some(second) = positional.get(1) {
        probe.clone_from(second);
    }
    Arguments { build, probe, out, work_dir }
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
    let build_path = arguments.build.clone();
    let probe_path = arguments.probe.clone();
    let work_dir = arguments.work_dir.clone();

    println!("build side: {}", build_path.display());
    println!("probe side: {}", probe_path.display());
    println!("iterations: {ITERATIONS}\n");

    std::fs::create_dir_all(&work_dir).expect("the work directory is created");

    // The published file's six row groups, written as six files.
    let batches = read_source(&build_path);
    let files = write_files(&batches, &work_dir.join("files"));
    println!("--- the build side ---");
    println!("one file: {}", build_path.display());
    println!("six files: {}", files.len());
    for file in &files {
        println!("  {}", file.display());
    }
    println!();

    // The probe side, opened once: it is the same in every pass.
    let probe_source = open_source(&probe_path).expect("the probe side opens");

    // --- the one-file build -------------------------------------------------
    let one_plan = plan(vec![build_path.clone()]);
    let mut one_builds = Vec::with_capacity(ITERATIONS);
    let mut one_opens = Vec::with_capacity(ITERATIONS);
    let mut one = None;
    for _ in 0..ITERATIONS {
        let pass = run(&probe_source, &one_plan);
        one_builds.push(pass.build);
        one_opens.push(pass.open);
        one = Some(pass);
    }
    let one = one.expect("the probe ran at least once");

    // --- the six-file build -------------------------------------------------
    let set_plan = plan(files.clone());
    let mut set_builds = Vec::with_capacity(ITERATIONS);
    let mut set_opens = Vec::with_capacity(ITERATIONS);
    let mut set = None;
    for _ in 0..ITERATIONS {
        let pass = run(&probe_source, &set_plan);
        set_builds.push(pass.build);
        set_opens.push(pass.open);
        set = Some(pass);
    }
    let set = set.expect("the probe ran at least once");

    // --- the table's shape --------------------------------------------------
    let one_source = ParquetSource::open(&build_path).expect("the build side opens");
    let set_source = FileSetSource::open(&files).expect("the set opens");
    let one_range = one_source.column_range("l_orderkey");
    let set_range = set_source.column_range("l_orderkey");
    let one_width = one_range.as_ref().and_then(|(min, max)| width(min, max));
    let set_width = set_range.as_ref().and_then(|(min, max)| width(min, max));
    let one_direct = one_width.map(direct_table_fits);
    let set_direct = set_width.map(direct_table_fits);

    // --- the estimate -------------------------------------------------------
    let one_estimate = estimate_build_side(&one_source, "l_orderkey", &[], 1);
    let set_estimate = estimate_build_side_set(&set_source, "l_orderkey", &[], 1);

    let one_build_timing = Timing::of(&one_builds);
    let one_open_timing = Timing::of(&one_opens);
    let set_build_timing = Timing::of(&set_builds);
    let set_open_timing = Timing::of(&set_opens);

    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    let mut put = |name: &str, timing: &Timing| {
        medians.insert(name.to_owned(), serde_json::json!(timing.median));
        spreads.insert(name.to_owned(), serde_json::json!([timing.min, timing.max]));
    };
    put("build_one_file", &one_build_timing);
    put("open_one_file", &one_open_timing);
    put("build_six_files", &set_build_timing);
    put("open_six_files", &set_open_timing);

    print_the_measurements(
        &medians,
        &one,
        &set,
        one_width,
        set_width,
        one_direct,
        set_direct,
        one_estimate,
        set_estimate,
    );

    let report = report(
        &build_path,
        &probe_path,
        &files,
        &one,
        &set,
        one_width,
        set_width,
        one_direct,
        set_direct,
        one_estimate,
        set_estimate,
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
    one: &Pass,
    set: &Pass,
    one_width: Option<u64>,
    set_width: Option<u64>,
    one_direct: Option<bool>,
    set_direct: Option<bool>,
    one_estimate: usize,
    set_estimate: usize,
) {
    println!("--- the build side's cost ---");
    print_timings(
        medians,
        &["build_one_file", "open_one_file", "build_six_files", "open_six_files"],
    );
    println!();
    println!("--- the identity ---");
    println!("rows:    {} against {}", one.rows, set.rows);
    println!("answer:  {} against {}", one.answer, set.answer);
    // The answer is compared **bit for bit**: the claim is that the two build
    // sides give the same answer, not that they are close.
    let same = one.rows == set.rows && one.answer.to_bits() == set.answer.to_bits();
    println!("the same rows: {same}");
    println!();
    println!("--- the table's shape ---");
    println!("one file's range width: {one_width:?}");
    println!("six files' range width: {set_width:?}");
    println!("one file's direct table: {one_direct:?}");
    println!("six files' direct table: {set_direct:?}");
    println!();
    println!("--- the estimate ---");
    println!("one file: {one_estimate} bytes");
    println!("six files: {set_estimate} bytes");
}

/// The report, for the committed raw report.
#[allow(clippy::too_many_arguments)]
fn report(
    build_file: &Path,
    probe_file: &Path,
    files: &[PathBuf],
    one: &Pass,
    set: &Pass,
    one_width: Option<u64>,
    set_width: Option<u64>,
    one_direct: Option<bool>,
    set_direct: Option<bool>,
    one_estimate: usize,
    set_estimate: usize,
    medians: &serde_json::Map<String, serde_json::Value>,
    spreads: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "probe": "build-set",
        "build_file": build_file.display().to_string(),
        "probe_file": probe_file.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "files": files.len(),
        "rows_one_file": one.rows,
        "rows_six_files": set.rows,
        "answer_one_file": one.answer,
        "answer_six_files": set.answer,
        "identical": one.rows == set.rows && one.answer.to_bits() == set.answer.to_bits(),
        "range_one_file": one_width,
        "range_six_files": set_width,
        "direct_one_file": one_direct,
        "direct_six_files": set_direct,
        "estimate_one_file": one_estimate,
        "estimate_six_files": set_estimate,
        "plan_estimate_one_file": one.estimate,
        "plan_estimate_six_files": set.estimate,
        "measurements_ms": medians,
        "spreads_ms": spreads,
    })
}

/// The width of a range, in keys.
fn width(min: &ScalarValue, max: &ScalarValue) -> Option<u64> {
    match (min, max) {
        (ScalarValue::Int64(min), ScalarValue::Int64(max)) => {
            let span = i128::from(*max) - i128::from(*min) + 1;
            u64::try_from(span).ok()
        }
        _ => None,
    }
}
