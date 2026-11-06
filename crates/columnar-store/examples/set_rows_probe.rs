//! The set's rows: a store's table, read by the engine and written back.
//!
//! ```text
//! cargo run --release -p columnar-store --example set_rows_probe -- \
//!     data/lineitem.parquet --out bench-results/set-rows-run1.json
//! ```
//!
//! # The question
//!
//! Spec 042 gave the engine a way to hand **a file's** rows out — `execute_rows_on` — and the store a way
//! to take them — `append_rows`. **The set's own rows were still folded into an aggregate**, so a store's
//! table could not be read by the engine and written back.
//!
//! Spec 043 gives the set its own door: `FileSetSource::execute_rows`. **This probe says what that door
//! costs, and what comes out of it.**
//!
//! # The experiment
//!
//! The published file's own **six row groups** are written as **six files** — the experiment spec 039 used
//! and spec 042 reused — and the round trip is run twice in the same campaign: once over the **one file**,
//! and once over the **six files**. The rows are the same rows, so the only thing that moves is the set.
//!
//! # The four questions
//!
//! | | |
//! |---|---|
//! | **the cost** | the read of the set against the read of the one file, and the write of each one's rows |
//! | **the identity** | the two files the two round trips write, compared byte for byte |
//! | **the round trip** | the file the round trip writes, against the published file: the bytes, and the first differing byte |
//! | **the estimate** | what the budget is charged for the set, against what the read used |
//!
//! **The round trip is written twice**, because the store's own schema rule and the file's own schema are
//! two different files, and the difference between them is the rule and nothing else. **The first differing
//! byte is named, whichever way it comes out.**

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::RecordBatchReader;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use columnar_engine::memory::estimate_lazy_read_for;
use columnar_engine::{
    execute_rows_on, open_source, ExecOptions, FileSetSource, MemoryBudget, QueryPlan,
};
use columnar_store::rows;
use columnar_store::{writer_properties, Store, BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::reader::FileReader;
use parquet::schema::types::ColumnPath;

/// The rows in a row group, and the grid the files are written on.
const ROW_GROUP_ROWS: usize = 1_000_000;

/// The buffer the writer's output goes through.
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;

/// The one column the published file writes plain.
const COMMENT_COLUMN: &str = "l_comment";

/// How many times each measurement is repeated.
const ITERATIONS: usize = 3;

/// The budget the round trip is given.
///
/// **The whole table is held in memory twice** — once as the engine's rows and once as the store's
/// batches — so the default 256 MiB would refuse the plan before it read a row. **The budget is not a
/// figure here**: the estimate is published beside it, and the estimate is what the budget check is made
/// of.
const BUDGET_BYTES: usize = 16 * 1024 * 1024 * 1024;

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
                .replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
                .unwrap_or(0.0)
        },
    )
}

/// The published file's own writer properties.
fn published_properties() -> WriterProperties {
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

/// The file's own rows, read once, in the store's grid.
fn read_source(path: &Path) -> Vec<RecordBatch> {
    let file = File::open(path).expect("the source opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let reader = builder.with_batch_size(BATCH_ROWS).build().expect("the reader");
    reader.map(|batch| batch.expect("the batch")).collect()
}

/// The file's own schema, read from the file.
fn source_schema(path: &Path) -> SchemaRef {
    let file = File::open(path).expect("the source opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    builder.build().expect("the reader").schema()
}

/// Writes the rows as one file per row group, and returns the files.
///
/// The rows are the published file's own rows, in the published file's own batches, so **the only thing
/// that changes is the number of files**.
fn write_files(batches: &[RecordBatch], dir: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("the directory is created");
    let properties = published_properties();
    let mut files = Vec::new();
    let mut index = 0_usize;
    let mut rows_in_file = 0_usize;
    let mut writer: Option<ArrowWriter<std::io::BufWriter<File>>> = None;

    for batch in batches {
        let mut offset = 0_usize;
        loop {
            if writer.is_none() {
                let path = dir.join(format!("{index:06}.parquet"));
                let file = File::create(&path).expect("the file is created");
                let buffered = std::io::BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
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

/// The plan the round trip runs: **every row, every column**.
///
/// The projection is empty, so the plan's rows carry every column, and there is no filter, so every row
/// survives. **The rows are the table's rows.**
fn plan() -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: None,
        join: None,
        sort: None,
    }
}

/// The options the round trip runs with.
fn options() -> ExecOptions {
    ExecOptions { memory_budget: MemoryBudget { bytes: BUDGET_BYTES }, flavor: None, threads: None }
}

/// The file's own schema, with the dictionary materialised away.
///
/// **The engine's rows are gathered, and the gather materialises the dictionary away**: the rows are
/// plain columns where the file's own schema says a dictionary. This is the schema that describes them —
/// the file's own schema, with that one difference, and the file's own nullability.
fn gathered_schema(schema: &SchemaRef) -> SchemaRef {
    let fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| {
            let data_type = match field.data_type() {
                DataType::Dictionary(_, value) => value.as_ref().clone(),
                other => other.clone(),
            };
            Field::new(field.name(), data_type, field.is_nullable())
        })
        .collect();
    Arc::new(Schema::new(fields))
}

/// Compares two files byte for byte, and says where they first differ.
///
/// **The first differing byte is the first byte that differs.** The files may be different lengths, and
/// when one of them ends before the other, the first difference is the byte the shorter file does not
/// have -- which is the end of the shorter file's data, and where the other file's metadata begins.
fn compare_files(left: &Path, right: &Path) -> (bool, Option<u64>) {
    let mut left_reader = BufReader::new(File::open(left).expect("the left file opens"));
    let mut right_reader = BufReader::new(File::open(right).expect("the right file opens"));
    let mut left_chunk = vec![0u8; 64 * 1024];
    let mut right_chunk = vec![0u8; 64 * 1024];
    let mut offset = 0u64;
    loop {
        let left_read = left_reader.read(&mut left_chunk).expect("the left file reads");
        let right_read = right_reader.read(&mut right_chunk).expect("the right file reads");
        let common = left_read.min(right_read);
        if left_chunk[..common] != right_chunk[..common] {
            let differing = left_chunk[..common]
                .iter()
                .zip(right_chunk[..common].iter())
                .position(|(left, right)| left != right)
                .unwrap_or(0);
            return (false, Some(offset + differing as u64));
        }
        offset += common as u64;
        if left_read != right_read {
            return (false, Some(offset));
        }
        if left_read == 0 {
            return (true, None);
        }
    }
}

/// Where a Parquet file's metadata starts.
///
/// A Parquet file ends with the metadata, its length as four little-endian bytes, and the four-byte magic.
/// **Everything before that is the file's data.**
fn metadata_start(path: &Path) -> u64 {
    let file = File::open(path).expect("the file opens");
    let size = file.metadata().expect("the file has a size").len();
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::End(-8)).expect("the reader seeks");
    let mut tail = [0u8; 8];
    reader.read_exact(&mut tail).expect("the tail reads");
    let metadata_len = u64::from(u32::from_le_bytes(tail[..4].try_into().expect("four bytes")));
    size - 8 - metadata_len
}

/// The Parquet schema, as the file's metadata holds it.
fn parquet_schema(path: &Path) -> String {
    let file = File::open(path).expect("the file opens");
    let reader = parquet::file::serialized_reader::SerializedFileReader::new(file)
        .expect("the reader is made");
    format!("{:?}", reader.metadata().file_metadata().schema_descr())
}

/// The Arrow schema the Parquet writer keeps in the file's footer.
///
/// The writer base64-encodes the IPC schema and stores it under the key
/// `ARROW:schema`, so **the two files' entries say whether the schemas are the same** -- and the schema is
/// where a dictionary column and a plain one differ.
fn arrow_schema(path: &Path) -> Option<String> {
    let file = File::open(path).expect("the file opens");
    let reader = parquet::file::serialized_reader::SerializedFileReader::new(file)
        .expect("the reader is made");
    reader.metadata().file_metadata().key_value_metadata().and_then(|entries| {
        entries
            .iter()
            .find(|entry| entry.key == "ARROW:schema")
            .and_then(|entry| entry.value.clone())
    })
}

/// Whether two files hold the same bytes in front of their metadata.
///
/// **This is the question the first differing byte cannot answer on its own**: a difference that is in the
/// metadata leaves the data alone, and a difference that is in the data does not.
fn data_regions_match(left: &Path, right: &Path) -> bool {
    let mut left_left = metadata_start(left);
    let mut right_left = metadata_start(right);
    let mut left_reader = BufReader::new(File::open(left).expect("the left file opens"));
    let mut right_reader = BufReader::new(File::open(right).expect("the right file opens"));
    let mut left_chunk = vec![0u8; 64 * 1024];
    let mut right_chunk = vec![0u8; 64 * 1024];
    while left_left > 0 && right_left > 0 {
        let left_take = usize::try_from(left_left).unwrap_or(usize::MAX);
        let right_take = usize::try_from(right_left).unwrap_or(usize::MAX);
        let take = (64 * 1024).min(left_take).min(right_take);
        let left_read = left_reader.read(&mut left_chunk[..take]).expect("the left file reads");
        let right_read = right_reader.read(&mut right_chunk[..take]).expect("the right file reads");
        let common = left_read.min(right_read);
        if left_chunk[..common] != right_chunk[..common] {
            return false;
        }
        left_left -= left_read as u64;
        right_left -= right_read as u64;
    }
    left_left == 0 && right_left == 0
}

/// The file's size in bytes.
fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// Writes the engine's rows into the store, and returns the file the store wrote.
///
/// **The store's own rule**: the schema is the store's, and the store's rule says a column that holds no
/// NULL is written REQUIRED.
fn write_rows(store: &Store, name: &str, rows: &[columnar_core::ColumnChunk]) -> PathBuf {
    let manifest = store
        .append_rows(name, rows, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let file = manifest.files.last().expect("the manifest names a file").name.clone();
    store.file_path(name, &file)
}

/// Writes the engine's rows into the store under the file's own schema, and returns the file.
///
/// **The file's own nullability**, with the dictionary materialised away: the schema that describes the
/// rows the engine handed out.
fn write_rows_under(
    store: &Store,
    name: &str,
    rows: &[columnar_core::ColumnChunk],
    schema: &SchemaRef,
) -> PathBuf {
    let arrays = rows::arrays(rows).expect("the chunks become arrays");
    let total = rows.first().map_or(0, |chunk| chunk.len);
    let mut batches = Vec::new();
    let mut start = 0;
    while start < total {
        let len = BATCH_ROWS.min(total - start);
        let columns = arrays.iter().map(|array| array.slice(start, len)).collect();
        batches.push(RecordBatch::try_new(schema.clone(), columns).expect("the batch is made"));
        start += len;
    }
    let manifest = store
        .append_with(name, &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let file = manifest.files.last().expect("the manifest names a file").name.clone();
    store.file_path(name, &file)
}

/// What one arm of the round trip produced.
#[derive(Debug)]
struct Arm {
    /// The rows the engine handed out.
    rows: usize,
    /// The file the store wrote, under the store's own rule.
    rule: PathBuf,
    /// The file the store wrote, under the file's own schema.
    own: PathBuf,
}

/// One arm of the round trip: the read, the two writes, and the facts.
fn arm(
    read: &mut dyn FnMut() -> columnar_engine::Dataset,
    store_rule: &Store,
    store_own: &Store,
    schema: &SchemaRef,
) -> (Arm, Duration, Duration) {
    let started = Instant::now();
    let rows = read();
    let read_elapsed = started.elapsed();

    let started = Instant::now();
    let rule = write_rows(store_rule, "round-trip", &rows.chunks);
    let write_elapsed = started.elapsed();

    let own = write_rows_under(store_own, "round-trip", &rows.chunks, schema);
    let count = rows.rows;
    (Arm { rows: count, rule, own }, read_elapsed, write_elapsed)
}

/// The engine's eager read of the published file, and the rows it hands out.
///
/// **This is the decomposition of the read.** The lazy path's read is the scan **and the gather**, and the
/// gather copies every column of the table: the rows the engine hands out are a copy of what it read. The
/// eager path's read is the decode alone, and the eager path's rows are the decode **and the same
/// gather** -- so the difference between them is the gather, measured on the same rows.
fn eager_pair(file: &Path) -> (Duration, Duration) {
    let started = Instant::now();
    let dataset = columnar_engine::Dataset::read_parquet(file).expect("the engine reads the file");
    let read_elapsed = started.elapsed();

    let started = Instant::now();
    let _rows =
        columnar_engine::execute_rows(&plan(), &dataset, &options()).expect("the rows come out");
    (read_elapsed, started.elapsed())
}

/// The probe's arguments.
struct Arguments {
    /// The file the round trip starts from.
    file: PathBuf,
    /// Where the report goes.
    out: Option<String>,
    /// Where the work goes.
    work_dir: PathBuf,
    /// Whether the work directory survives the probe.
    keep: bool,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut arguments = std::env::args().skip(1);
    let mut file = PathBuf::from("data/lineitem.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/set-rows-probe");
    let mut keep = false;
    let mut positional = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => out = arguments.next(),
            "--keep" => keep = true,
            "--work-dir" => {
                if let Some(value) = arguments.next() {
                    work_dir = PathBuf::from(value);
                }
            }
            _ => positional.push(PathBuf::from(argument)),
        }
    }
    if let Some(first) = positional.first() {
        file.clone_from(first);
    }
    Arguments { file, out, work_dir, keep }
}

/// The campaign's samples: every measurement, every iteration.
#[derive(Debug, Default)]
struct Campaign {
    /// The one file's rows.
    read_one: Vec<Duration>,
    /// The six files' rows.
    read_six: Vec<Duration>,
    /// The write of the one file's rows.
    write_one: Vec<Duration>,
    /// The write of the six files' rows.
    write_six: Vec<Duration>,
    /// The engine's eager read.
    eager_reads: Vec<Duration>,
    /// The eager rows: the read and the gather.
    eager_rows: Vec<Duration>,
    /// What the round trips produced.
    facts: Facts,
    /// The difference between the two arms, in every iteration.
    differences: Vec<Difference>,
}

/// The difference between the two arms, in one iteration.
///
/// **This is the yardstick for a pair**: the two arms are measured back to back, so the host's movement is
/// in both sides of every one of these numbers.
#[derive(Debug, Clone, Copy)]
struct Difference {
    /// The set's rows against the one file's rows.
    read: f64,
    /// The write of the set's rows against the write of the one file's rows.
    write: f64,
}

/// Runs the campaign: three iterations, each with both arms.
fn campaign(
    file: &Path,
    work_dir: &Path,
    one_source: &columnar_engine::ParquetSource,
    set: &FileSetSource,
    gathered: &SchemaRef,
) -> Campaign {
    let mut campaign = Campaign::default();
    for iteration in 0..ITERATIONS {
        let store_rule =
            Store::open(work_dir.join(format!("rule-{iteration}"))).expect("the store opens");
        let store_own =
            Store::open(work_dir.join(format!("own-{iteration}"))).expect("the store opens");

        let (eager_read, eager_gathered) = eager_pair(file);
        campaign.eager_reads.push(eager_read);
        campaign.eager_rows.push(eager_gathered);

        // **The order alternates.** The two arms are measured back to back in
        // every iteration, and the order they are measured in flips from one
        // iteration to the next, so the host's own drift does not favour one
        // arm over the other.
        let (one, one_read, one_write, six, six_read, six_write) = if iteration % 2 == 0 {
            let one = arm(
                &mut || {
                    execute_rows_on(&plan(), one_source, &options()).expect("the rows come out")
                },
                &store_rule,
                &store_own,
                gathered,
            );
            let six = arm(
                &mut || set.execute_rows(&plan(), &options()).expect("the rows come out"),
                &store_rule,
                &store_own,
                gathered,
            );
            (one.0, one.1, one.2, six.0, six.1, six.2)
        } else {
            let six = arm(
                &mut || set.execute_rows(&plan(), &options()).expect("the rows come out"),
                &store_rule,
                &store_own,
                gathered,
            );
            let one = arm(
                &mut || {
                    execute_rows_on(&plan(), one_source, &options()).expect("the rows come out")
                },
                &store_rule,
                &store_own,
                gathered,
            );
            (one.0, one.1, one.2, six.0, six.1, six.2)
        };

        campaign.read_one.push(one_read);
        campaign.read_six.push(six_read);
        campaign.write_one.push(one_write);
        campaign.write_six.push(six_write);
        campaign.differences.push(Difference {
            read: ms(six_read) - ms(one_read),
            write: ms(six_write) - ms(one_write),
        });

        campaign.facts.collect(&one, &six, file);
        println!(
            "iteration {iteration}: read {:>8.3} / {:>8.3} ms, write {:>8.3} / {:>8.3} ms, \
             the differences {:>+8.3} / {:>+8.3} ms",
            ms(one_read),
            ms(six_read),
            ms(one_write),
            ms(six_write),
            ms(six_read) - ms(one_read),
            ms(six_write) - ms(one_write)
        );
    }
    campaign
}

/// The medians and the spreads, for the report and for the printout.
fn summaries(
    campaign: &Campaign,
) -> (serde_json::Map<String, serde_json::Value>, serde_json::Map<String, serde_json::Value>) {
    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    let mut put = |name: &str, timing: &Timing| {
        medians.insert(name.to_owned(), serde_json::json!(timing.median));
        spreads.insert(name.to_owned(), serde_json::json!([timing.min, timing.max]));
    };
    put("eager_read", &Timing::of(&campaign.eager_reads));
    put("eager_rows", &Timing::of(&campaign.eager_rows));
    put("read_one_file", &Timing::of(&campaign.read_one));
    put("read_six_files", &Timing::of(&campaign.read_six));
    put("write_one_file", &Timing::of(&campaign.write_one));
    put("write_six_files", &Timing::of(&campaign.write_six));
    (medians, spreads)
}

/// What the budget is charged for the one file, and for the set.
fn estimates(one_source: &columnar_engine::ParquetSource, set: &FileSetSource) -> (usize, usize) {
    let one = estimate_lazy_read_for(
        one_source.rows(),
        one_source,
        &(0..one_source.columns().len()).collect::<Vec<_>>(),
        1,
        false,
    );
    let six = estimate_lazy_read_for(
        set.rows(),
        set,
        &(0..set.columns().len()).collect::<Vec<_>>(),
        1,
        false,
    );
    (one, six)
}

fn main() {
    let arguments = arguments();
    let file = arguments.file.clone();
    let work_dir = arguments.work_dir.clone();

    println!("the file: {}", file.display());
    println!("iterations: {ITERATIONS}\n");

    std::fs::create_dir_all(&work_dir).expect("the work directory is created");

    // The published file's six row groups, written as six files.
    let batches = read_source(&file);
    let files = write_files(&batches, &work_dir.join("files"));
    println!("--- the table ---");
    println!("one file: {}", file.display());
    println!("six files: {}", files.len());
    println!();

    let gathered = gathered_schema(&source_schema(&file));
    let one_source = open_source(&file).expect("the one file opens");
    let set = FileSetSource::open(&files).expect("the set opens");

    let campaign = campaign(&file, &work_dir, &one_source, &set, &gathered);
    let (medians, spreads) = summaries(&campaign);
    print_the_measurements(&medians, &campaign.facts);

    let (estimate_one, estimate_six) = estimates(&one_source, &set);
    let report = report(
        &file,
        &files,
        &campaign.facts,
        estimate_one,
        estimate_six,
        &medians,
        &spreads,
        &campaign.differences,
    );
    println!();
    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    if arguments.keep {
        println!("the work directory survives: {}", work_dir.display());
    } else {
        std::fs::remove_dir_all(&work_dir).ok();
    }
}

/// The facts the round trip produced.
///
/// **The verdicts are independent**: each says whether two files are the same, and
/// they do not exclude one another, so the struct holds more than three of them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
struct Facts {
    /// The rows the two arms handed out.
    rows_one: usize,
    /// The rows the set handed out.
    rows_six: usize,
    /// Whether the two arms wrote the same file.
    identical_arms: bool,
    /// Whether the round trip wrote the published file.
    identical_to_published: bool,
    /// The first differing byte against the published file.
    first_difference: Option<u64>,
    /// Whether the round trip under the file's own schema wrote the published file.
    identical_own: bool,
    /// The first differing byte under the file's own schema.
    first_difference_own: Option<u64>,
    /// Whether the store's rule wrote the published file's data.
    data_identical_rule: bool,
    /// Whether the file's own schema wrote the published file's data.
    data_identical_own: bool,
    /// Whether the two files keep the same Arrow schema in their footers.
    arrow_schema_identical_own: bool,
    /// Whether the two files keep the same Parquet schema.
    parquet_schema_identical_own: bool,
    /// Where the published file's metadata starts.
    metadata_start: u64,
    /// The bytes the store's rule wrote.
    bytes_rule: u64,
    /// The bytes the file's own schema wrote.
    bytes_own: u64,
}

impl Facts {
    /// Collects the facts of one iteration.
    fn collect(&mut self, one: &Arm, six: &Arm, published: &Path) {
        self.rows_one = one.rows;
        self.rows_six = six.rows;
        let (same, _difference) = compare_files(&one.rule, &six.rule);
        self.identical_arms = same;
        let (published_same, published_difference) = compare_files(&one.rule, published);
        self.identical_to_published = published_same;
        self.first_difference = published_difference;
        let (own_same, own_difference) = compare_files(&one.own, published);
        self.identical_own = own_same;
        self.first_difference_own = own_difference;
        self.data_identical_rule = data_regions_match(&one.rule, published);
        self.data_identical_own = data_regions_match(&one.own, published);
        self.arrow_schema_identical_own = arrow_schema(&one.own) == arrow_schema(published);
        self.parquet_schema_identical_own = parquet_schema(&one.own) == parquet_schema(published);
        self.metadata_start = metadata_start(published);
        self.bytes_rule = file_bytes(&one.rule);
        self.bytes_own = file_bytes(&one.own);
    }
}

/// The measurements, printed.
fn print_the_measurements(medians: &serde_json::Map<String, serde_json::Value>, facts: &Facts) {
    println!("--- the cost ---");
    for name in [
        "eager_read",
        "eager_rows",
        "read_one_file",
        "read_six_files",
        "write_one_file",
        "write_six_files",
    ] {
        let median = medians.get(name).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        println!("{name:<18}: {median:>10.3} ms");
    }
    let gather = medians.get("eager_rows").and_then(serde_json::Value::as_f64).unwrap_or(0.0)
        - medians.get("eager_read").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    println!("the gather       : {gather:>10.3} ms");
    println!();
    println!("--- the identity ---");
    println!("the rows: {} against {}", facts.rows_one, facts.rows_six);
    println!("the two files are the same: {}", facts.identical_arms);
    println!();
    println!("--- the round trip ---");
    println!("the store's rule: {} bytes", facts.bytes_rule);
    println!("the file's own schema: {} bytes", facts.bytes_own);
    println!("the published file: {}", facts.identical_to_published);
    println!("the first differing byte: {:?}", facts.first_difference);
    println!("the file's own schema is the published file: {}", facts.identical_own);
    println!("the first differing byte, own schema: {:?}", facts.first_difference_own);
    println!("the data, the store's rule: {}", facts.data_identical_rule);
    println!("the data, the file's own schema: {}", facts.data_identical_own);
    println!("the Arrow schema is the same: {}", facts.arrow_schema_identical_own);
    println!("the Parquet schema is the same: {}", facts.parquet_schema_identical_own);
    println!("the published file's metadata starts at: {}", facts.metadata_start);
}

/// The report, for the committed raw report.
#[allow(clippy::too_many_arguments)]
fn report(
    file: &Path,
    files: &[PathBuf],
    facts: &Facts,
    estimate_one: usize,
    estimate_six: usize,
    medians: &serde_json::Map<String, serde_json::Value>,
    spreads: &serde_json::Map<String, serde_json::Value>,
    differences: &[Difference],
) -> serde_json::Value {
    let difference_rows: Vec<serde_json::Value> = differences
        .iter()
        .map(|difference| serde_json::json!({"read": difference.read, "write": difference.write}))
        .collect();
    serde_json::json!({
        "probe": "set-rows",
        "file": file.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "files": files.len(),
        "rows_one_file": facts.rows_one,
        "rows_six_files": facts.rows_six,
        "identical_arms": facts.identical_arms,
        "identical_to_published": facts.identical_to_published,
        "first_difference": facts.first_difference,
        "identical_own_schema": facts.identical_own,
        "first_difference_own_schema": facts.first_difference_own,
        "data_identical_rule": facts.data_identical_rule,
        "data_identical_own_schema": facts.data_identical_own,
        "arrow_schema_identical_own_schema": facts.arrow_schema_identical_own,
        "parquet_schema_identical_own_schema": facts.parquet_schema_identical_own,
        "metadata_start": facts.metadata_start,
        "bytes_rule": facts.bytes_rule,
        "bytes_own_schema": facts.bytes_own,
        "estimate_one_file": estimate_one,
        "estimate_six_files": estimate_six,
        "measurements_ms": medians,
        "spreads_ms": spreads,
        "differences_ms": difference_rows,
    })
}
