//! The index's shape and its price.
//!
//! ```text
//! cargo run --release -p columnar-db --example index_probe -- \
//!     --data data/lineitem.parquet --out bench-results/index-cost-run1.json
//! ```
//!
//! # The question
//!
//! **The index's shape is decided by a figure, and the price is missing.**
//! [ADR-0066](../../../docs/adr/0066-the-databases-own-price.md) gave the rule
//! -- *a point lookup that a scan already answers more cheaply is not given an
//! index, and the database says so* -- and the figure: the point lookup costs
//! **43.319 ms** as a scan and **2.657 ms** over the one file an index would
//! reach. **And the index's own price -- its build, its storage and its
//! lookup -- is unmeasured, because there is no index to measure.**
//!
//! **So the shape is measured first, and this probe is the price.** The shape
//! is in [§3am](../../../PERF.md#3am-the-indexs-shape-measured-before-it-is-built):
//! the range map names every file and is refused, and the per-key map narrows.
//! **And this probe builds that map, in the snapshot, and measures what it
//! costs** -- on every axis the rule turns on.
//!
//! # What the probe measures
//!
//! | | |
//! |---|---|
//! | **the key's order** | whether the key rises in the table's rows, and how often it does not |
//! | **each file's range** | the key's minimum and maximum in each of the table's files |
//! | **the distinct keys** | how many entries the per-key map holds |
//! | **the files each map names** | over a sample of keys, how many files each map would read |
//! | **the scan the index replaces** | the point lookup as a scan, through the database |
//! | **the floor** | the same lookup over the one file an index would reach |
//! | **the first lookup** | the index's build, and the read |
//! | **the second lookup** | the read, with the index built |
//! | **the write's cost** | one grid's worth of rows, with no index alive and with one |
//! | **the compaction's cost** | with an index alive and without one |
//! | **the compacted table** | the scan and the two lookups, after the compaction |
//!
//! **And every difference is a difference of two measurements taken back to
//! back**, so the machine's movement cannot land on one side of it.
//!
//! # The table
//!
//! **The same table the database's own probe builds**: a million rows of the
//! published lineitem file -- three columns, written on the store's grid, so
//! **sixteen files of 65,536 rows** -- and the same key column, so the numbers
//! here and the numbers in [PERF.md §3ak](../../../PERF.md#3ak-the-databases-own-price)
//! are about the same table.
//!
//! # And the memory is measured elsewhere
//!
//! **A time measured beside a counting allocator is not the engine's time.**
//! So the index's own bytes and its peak memory are measured in
//! `index_memory_probe.rs`, which has the allocator in it, and this probe has
//! no allocator in it.

#![allow(clippy::indexing_slicing, clippy::cast_precision_loss)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_db::prelude::*;
use columnar_engine::{
    Dataset, Encoding, ExecOptions, FileSetSource, ParquetSource, QueryPlan, ScalarValue,
};

/// The key column: the point lookup's column.
const KEY: &str = "l_orderkey";

/// The float column.
const VALUE: &str = "l_quantity";

/// The text column.
const TEXT: &str = "l_shipmode";

/// The table the probe writes.
const TABLE: &str = "many";

/// The grid: the store's own, and the rows' own batch.
const GRID: usize = columnar_store::BATCH_ROWS;

/// The rows the probe's table holds.
const DEFAULT_ROWS: usize = 1_000_000;

/// How many keys the maps are asked about: the table's rows at even intervals.
const SAMPLES: usize = 64;

/// How many times the probe runs its whole shape. **Each run builds a fresh
/// database**, so every measurement is taken over the files that run wrote.
const ITERATIONS: usize = 3;

/// One measurement: its name and how long it took.
type Reading = (&'static str, Duration);

/// The probe's arguments.
struct Arguments {
    /// The file the probe reads its rows from.
    data: PathBuf,
    /// Where the report goes.
    out: Option<String>,
    /// Where the database is built.
    work_dir: PathBuf,
    /// How many rows the probe's table holds.
    rows: usize,
}

/// The probe's arguments, from the command line.
fn arguments() -> Arguments {
    let mut data = PathBuf::from("data/lineitem.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/index-probe");
    let mut rows = DEFAULT_ROWS;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--data" => {
                if let Some(value) = args.next() {
                    data = PathBuf::from(value);
                }
            }
            "--out" => out = args.next(),
            "--work-dir" => work_dir = PathBuf::from(args.next().unwrap_or_default()),
            "--rows" => {
                if let Some(value) = args.next() {
                    rows = value.parse().unwrap_or(DEFAULT_ROWS);
                }
            }
            _ => {}
        }
    }
    Arguments { data, out, work_dir, rows }
}

/// The rows the probe writes: the three columns, in the columns' order.
struct Source {
    /// The key column.
    keys: Vec<i64>,
    /// The float column.
    values: Vec<f64>,
    /// The text column.
    texts: Vec<String>,
}

impl Source {
    /// The rows from `from` for `count` rows.
    fn rows(&self, from: usize, count: usize) -> Vec<Row> {
        (from..from + count)
            .map(|row| {
                Row::filled(vec![
                    ScalarValue::Int64(self.keys[row]),
                    ScalarValue::Float64(self.values[row]),
                    ScalarValue::Utf8(self.texts[row].clone()),
                ])
            })
            .collect()
    }
}

/// The table's schema: the three columns, the key among them, and no nulls.
fn schema() -> Schema {
    Schema::new(vec![
        Column::key(KEY, DataType::Int64),
        Column::new(VALUE, DataType::Float64, false),
        Column::new(TEXT, DataType::Utf8, false),
    ])
    .expect("the schema is one a table can hold")
}

/// Reads the three columns of the source file, through the engine.
fn read_source(path: &Path) -> Source {
    let source = FileSetSource::open(&[path.to_path_buf()]).expect("the source opens");
    let plan =
        QueryPlan::new(Vec::new(), vec![KEY.to_owned(), VALUE.to_owned(), TEXT.to_owned()], None);
    let dataset = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    Source {
        keys: ints_of(&dataset, KEY),
        values: floats_of(&dataset, VALUE),
        texts: texts_of(&dataset, TEXT),
    }
}

/// The integers of a column, whatever way the file stores them.
fn ints_of(dataset: &Dataset, name: &str) -> Vec<i64> {
    let chunk = dataset.chunk(name).expect("the file holds the column");
    match &chunk.encoding {
        Encoding::Int64 { values } => values.clone(),
        Encoding::Int64Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                *dict.get(position).expect("the index is in range")
            })
            .collect(),
        other => panic!("the column {name} is not an integer: {other:?}"),
    }
}

/// The doubles of a column, whatever way the file stores them.
fn floats_of(dataset: &Dataset, name: &str) -> Vec<f64> {
    let chunk = dataset.chunk(name).expect("the file holds the column");
    match &chunk.encoding {
        Encoding::Float64 { values } => values.clone(),
        Encoding::Float64Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                *dict.get(position).expect("the index is in range")
            })
            .collect(),
        other => panic!("the column {name} is not a double: {other:?}"),
    }
}

/// The strings of a column, whatever way the file stores them.
fn texts_of(dataset: &Dataset, name: &str) -> Vec<String> {
    let chunk = dataset.chunk(name).expect("the file holds the column");
    match &chunk.encoding {
        Encoding::Utf8 { values } => values.clone(),
        Encoding::Utf8Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                dict.get(position).expect("the index is in range").clone()
            })
            .collect(),
        other => panic!("the column {name} is not a string: {other:?}"),
    }
}

/// Writes the rows into a table, on the grid, and commits them.
fn write_table(database: &mut Database, name: &str, source: &Source, rows: usize) {
    let mut table = database.create_table(name, schema()).expect("the table is created");
    let mut written = 0;
    while written < rows {
        let count = GRID.min(rows - written);
        let batch = source.rows(written, count);
        table.insert(&batch).expect("the rows are accepted");
        written += count;
    }
    table.commit().expect("the rows are written");
}

/// Creates the table and writes its first grid's worth of rows.
///
/// **And the first write is not measured**: it creates the table, and the
/// catalog's own write is a once.
fn create_table_with_rows(database: &mut Database, source: &Source, rows: usize) {
    let mut table = database.create_table(TABLE, schema()).expect("the table is created");
    let batch = source.rows(0, rows.min(GRID));
    table.insert(&batch).expect("the rows are accepted");
    table.commit().expect("the rows are written");
}

/// Writes the rest of the table's rows, on the grid, and commits them.
///
/// **And the first batch is already written**, so the rows start at `from`.
fn write_rest(database: &mut Database, source: &Source, rows: usize, from: usize) {
    let mut table = database.table_mut(TABLE).expect("the table is there");
    let mut written = from;
    while written < rows {
        let count = GRID.min(rows - written);
        let batch = source.rows(written, count);
        table.insert(&batch).expect("the rows are accepted");
        written += count;
    }
    table.commit().expect("the rows are written");
}

/// Writes one grid's worth of rows, and commits them.
///
/// **And the time it returns is the write path's own**: the insert, and the
/// commit that writes the file. **And the rows are new ones** -- they are
/// past the table's own rows -- so the write adds a file and does not
/// duplicate a key.
fn measure_write(database: &mut Database, source: &Source, from: usize) -> Duration {
    let started = Instant::now();
    {
        let mut table = database.table_mut(TABLE).expect("the table is there");
        let batch = source.rows(from, GRID);
        table.insert(&batch).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    started.elapsed()
}

/// The table's files, as the manifest names them.
fn files_of(database: &Database, name: &str) -> Vec<PathBuf> {
    database.store().table(name).expect("the store answers").expect("the table is there").paths()
}

/// A file's own shape: its rows, and the range of the key in it.
struct FileShape {
    /// How many rows the file holds.
    rows: usize,
    /// The key's smallest value in the file.
    min: i64,
    /// The key's largest value in the file.
    max: i64,
    /// The file's keys, in the file's order.
    keys: Vec<i64>,
}

/// Reads the table's files, and each file's own shape.
///
/// **The range is the file's own statistics**: the engine reads them when it
/// opens the file, and they are what a range map would use.
fn file_shapes(files: &[PathBuf]) -> Vec<FileShape> {
    files
        .iter()
        .map(|path| {
            let source = ParquetSource::open(path).expect("the file opens");
            let rows = source.rows();
            let Some((ScalarValue::Int64(min), ScalarValue::Int64(max))) = source.column_range(KEY)
            else {
                panic!("the file's own statistics hold the key's range");
            };
            FileShape { rows, min, max, keys: keys_of(path) }
        })
        .collect()
}

/// The keys of one file, in the file's order.
fn keys_of(path: &Path) -> Vec<i64> {
    let source = FileSetSource::open(&[path.to_path_buf()]).expect("the file opens");
    let plan = QueryPlan::new(Vec::new(), vec![KEY.to_owned()], None);
    let dataset = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    ints_of(&dataset, KEY)
}

/// The per-key map: the files that hold each key.
fn key_map(shapes: &[FileShape]) -> HashMap<i64, Vec<usize>> {
    let mut map: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, shape) in shapes.iter().enumerate() {
        for key in &shape.keys {
            map.entry(*key).or_default().push(index);
        }
    }
    map
}

/// The files the range map names for a key: the files whose range holds it.
fn range_map_files(shapes: &[FileShape], key: i64) -> usize {
    shapes.iter().filter(|shape| shape.min <= key && key <= shape.max).count()
}

/// The files the per-key map names for a key.
fn key_map_files(map: &HashMap<i64, Vec<usize>>, key: i64) -> usize {
    map.get(&key).map_or(0, Vec::len)
}

/// How often the key falls, over the table's rows in the table's order.
fn descents(keys: &[i64]) -> usize {
    keys.windows(2).filter(|pair| pair[1] < pair[0]).count()
}

/// The sample of keys the maps are asked about: the table's rows at even intervals.
fn sample_keys(keys: &[i64], samples: usize) -> Vec<i64> {
    if keys.is_empty() {
        return Vec::new();
    }
    let step = (keys.len() / samples).max(1);
    keys.iter().step_by(step).take(samples).copied().collect()
}

/// The smallest of the files' minimums, and the largest of their maximums.
///
/// **The two are the range map's own reach**: a key between them is in every
/// file's range, and a key outside them is in none.
fn range_extremes(shapes: &[FileShape]) -> (i64, i64) {
    let smallest = shapes.iter().map(|shape| shape.min).min().unwrap_or(0);
    let largest = shapes.iter().map(|shape| shape.max).max().unwrap_or(0);
    (smallest, largest)
}

/// The smallest and the largest of a set of counts.
fn smallest_and_largest(counts: &[usize]) -> (usize, usize) {
    let Some(first) = counts.first() else {
        return (0, 0);
    };
    let mut smallest = *first;
    let mut largest = *first;
    for count in counts {
        smallest = smallest.min(*count);
        largest = largest.max(*count);
    }
    (smallest, largest)
}

/// The mean of a set of counts.
fn mean(counts: &[usize]) -> f64 {
    if counts.is_empty() {
        return 0.0;
    }
    let total: usize = counts.iter().sum();
    total as f64 / counts.len() as f64
}

/// The host's load average, for the report.
fn load_average() -> f64 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map_or(0.0, |output| {
        String::from_utf8_lossy(&output.stdout)
            .replace(['{', '}'], " ")
            .split_whitespace()
            .next()
            .and_then(|first| first.parse::<f64>().ok())
            .unwrap_or(0.0)
    })
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

/// The scan: the whole table, filtered by the key, through the database.
///
/// **And it is the scan the index replaces** — the same question, asked of
/// every file.
fn measure_scan(table: &SnapshotTable<'_>, key: i64) -> Duration {
    let started = Instant::now();
    let plan = table.plan().filter(col(KEY).eq(key)).build().expect("the plan is built");
    let rows = table.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    let _ = rows.len();
    started.elapsed()
}

/// The engine's own read of a set of files, filtered by the key.
///
/// **And the one file is the floor**: the engine's own read granularity,
/// measured, and the index cannot go below it.
fn measure_engine_read(files: &[PathBuf], key: i64) -> Duration {
    let started = Instant::now();
    let source = FileSetSource::open(files).expect("the source opens");
    let plan = QueryPlan::new(
        vec![Predicate::new(KEY, ComparisonOp::Eq, ScalarValue::Int64(key))],
        vec![KEY.to_owned(), VALUE.to_owned(), TEXT.to_owned()],
        None,
    );
    let rows = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    let _ = rows.len();
    started.elapsed()
}

/// The database's own point lookup: the index's answer, and the rows it holds.
fn measure_lookup(table: &SnapshotTable<'_>, key: i64) -> Duration {
    let started = Instant::now();
    let rows = table.lookup(key).expect("the lookup answers");
    let _ = rows.len();
    started.elapsed()
}

/// The compacted table's three lookups.
struct Compacted {
    /// The scan of the compacted table, the first time: cold.
    scan: Duration,
    /// The scan of the compacted table again: warm.
    scan_again: Duration,
    /// The first lookup: the index's build, and the read.
    first: Duration,
    /// The second lookup: the read, with the index built.
    indexed: Duration,
    /// The rows the compaction wrote.
    rows: u64,
}

/// The compaction's arm: the compaction's own time, and the compacted table's
/// lookups.
struct CompactionArm {
    /// The compaction's own time.
    compaction: Duration,
    /// The compacted table's lookups -- **and none when the arm does not build
    /// an index**, because the compacted table is the same table either way.
    compacted: Option<Compacted>,
}

/// The compaction of the table, with an index alive or without one.
///
/// **And the two arms differ in one thing**: the index. **And both hold a
/// snapshot**, so the files the compaction retires stay alive in both -- **and
/// the compaction's cost is the same either way, because the compaction never
/// reads the index.**
///
/// **And the arm that builds the index also measures the compacted table**:
/// the scan, and the two lookups -- **because a compacted table is one file,
/// and one file cannot be narrowed.**
fn measure_compaction(
    source: &Source,
    rows: usize,
    key: i64,
    work_dir: &Path,
    with_index: bool,
) -> CompactionArm {
    let root = work_dir.join("compaction");
    std::fs::remove_dir_all(&root).ok();
    let mut database = Database::open(&root).expect("the database opens");
    write_table(&mut database, TABLE, source, rows);

    // **The snapshot that holds the table's files**, and the index when the arm
    // says so.
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table(TABLE).expect("the table is there");
    if with_index {
        let _ = table.lookup(key).expect("the lookup answers");
    }

    let started = Instant::now();
    let rows = {
        let mut table = database.table_mut(TABLE).expect("the table is there");
        table.compact().expect("the table is compacted").rows
    };
    let compaction = started.elapsed();

    let compacted = if with_index {
        // **And the compacted table's own three lookups.**
        let snapshot = database.snapshot().expect("the snapshot is taken");
        let table = snapshot.table(TABLE).expect("the table is there");
        let scan = measure_scan(&table, key);
        let first = measure_lookup(&table, key);
        let indexed = measure_lookup(&table, key);
        let scan_again = measure_scan(&table, key);
        Some(Compacted { scan, scan_again, first, indexed, rows })
    } else {
        None
    };

    std::fs::remove_dir_all(&root).ok();
    CompactionArm { compaction, compacted }
}

/// The medians and the spreads of every measurement, across the iterations.
fn timings_of(
    iterations: &[Vec<Reading>],
) -> (serde_json::Map<String, serde_json::Value>, serde_json::Map<String, serde_json::Value>) {
    let mut medians = serde_json::Map::new();
    let mut spreads = serde_json::Map::new();
    let Some(first) = iterations.first() else {
        return (medians, spreads);
    };
    for (name, _) in first {
        let mut values: Vec<f64> = iterations
            .iter()
            .filter_map(|iteration| {
                iteration.iter().find(|(other, _)| other == name).map(|(_, took)| ms(*took))
            })
            .collect();
        if values.is_empty() {
            continue;
        }
        values.sort_by(f64::total_cmp);
        let lowest = values.first().copied().unwrap_or(0.0);
        let highest = values.last().copied().unwrap_or(0.0);
        medians.insert((*name).to_owned(), serde_json::json!(median_of(&values)));
        spreads.insert((*name).to_owned(), serde_json::json!([lowest, highest]));
    }
    (medians, spreads)
}

/// The median of a set of numbers.
fn median_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// Milliseconds, for printing and for the report.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The figures the prices are derived from.
///
/// **And every difference here is a difference of two measurements taken back
/// to back**, so the machine's movement cannot land on one side of it.
fn derived_of(medians: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let took = |name: &str| medians.get(name).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    let scan = took("lookup_scan");
    let scan_again = took("lookup_scan_again");
    let first = took("lookup_first");
    let indexed = took("lookup_indexed");
    let build = first - indexed;
    let saving = scan_again - indexed;
    let compacted_scan = took("compacted_scan");
    let compacted_scan_again = took("compacted_scan_again");
    let compacted_first = took("compacted_first");
    let compacted_indexed = took("compacted_indexed");
    let compacted_build = compacted_first - compacted_indexed;
    let compacted_saving = compacted_scan_again - compacted_indexed;
    serde_json::json!({
        "lookup_build_ms": build,
        "lookup_saving_ms": saving,
        "lookup_saving_cold_ms": scan - indexed,
        "lookup_break_even": if saving > 0.0 { build / saving } else { 0.0 },
        "lookup_indexed_ratio": scan_again / indexed.max(0.001),
        "write_tax_ms": took("write_with_index") - took("write_no_index"),
        "compaction_tax_ms": took("compaction_with_index") - took("compaction_without_index"),
        "compacted_build_ms": compacted_build,
        "compacted_saving_ms": compacted_saving,
        "compacted_saving_cold_ms": compacted_scan - compacted_indexed,
        "compacted_break_even": if compacted_saving > 0.0 {
            compacted_build / compacted_saving
        } else {
            0.0
        },
    })
}

/// One iteration's results: the timings, the shape, and the map.
struct Iteration {
    /// Every timing the iteration took.
    readings: Vec<Reading>,
    /// The table's files, and their own shapes.
    shapes: Vec<FileShape>,
    /// The probe's own map: the key to the files that hold it.
    map: HashMap<i64, Vec<usize>>,
    /// The rows the compaction wrote.
    compacted_rows: u64,
}

/// One iteration: a fresh database, the table, and every measurement.
fn measure_once(
    arguments: &Arguments,
    source: &Source,
    rows: usize,
    key: i64,
    swap: bool,
) -> Iteration {
    let root = arguments.work_dir.join("db");
    std::fs::remove_dir_all(&root).ok();
    let mut database = Database::open(&root).expect("the database opens");

    // **The table is created**, and its first grid's worth of rows is written.
    create_table_with_rows(&mut database, source, rows);

    // **And the write's cost with no index alive**: one grid's worth of rows,
    // and the commit that writes the file.
    let write_no_index = measure_write(&mut database, source, GRID);

    // **And the rest of the table** — and it starts **past** the grid's worth
    // the write's cost measured, so no row is written twice.
    write_rest(&mut database, source, rows, 2 * GRID);

    let files = files_of(&database, TABLE);
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table(TABLE).expect("the table is there");

    let mut readings = Vec::with_capacity(12);
    readings.push(("write_no_index", write_no_index));

    // **The scan**: the whole table, filtered by the key — the same snapshot,
    // so the retired set is found once and the two measurements differ only in
    // the files they read.
    let scan = measure_scan(&table, key);
    readings.push(("lookup_scan", scan));

    // **The floor**: the same lookup over one file.
    let floor = measure_engine_read(&files[..1], key);
    readings.push(("lookup_one_file", floor));

    // **The first lookup**: the index's build, and the read.
    let first = measure_lookup(&table, key);
    readings.push(("lookup_first", first));

    // **The second lookup**: the read, with the index built.
    let indexed = measure_lookup(&table, key);
    readings.push(("lookup_indexed", indexed));

    // **And the scan again**: the same scan, over files the first scan has
    // already read -- **so the comparison the rule turns on is between two
    // reads of warm files**, and not between a cold scan and a warm lookup.
    let scan_again = measure_scan(&table, key);
    readings.push(("lookup_scan_again", scan_again));

    // **And the write's cost with the index alive**: the same work, the same
    // commit, and an index in a live snapshot that the write never touches.
    let write_with_index = measure_write(&mut database, source, rows);
    readings.push(("write_with_index", write_with_index));

    // **And the compaction's two arms**: with an index alive and without one.
    // **And the arms take turns**, because the first compaction in a process
    // pays for the allocator's own growth -- **and an order that never
    // changes would put that cost on one arm every time.**
    let (with_index, without_index) = if swap {
        (
            measure_compaction(source, rows, key, &arguments.work_dir, false),
            measure_compaction(source, rows, key, &arguments.work_dir, true),
        )
    } else {
        (
            measure_compaction(source, rows, key, &arguments.work_dir, true),
            measure_compaction(source, rows, key, &arguments.work_dir, false),
        )
    };
    readings.push(("compaction_with_index", with_index.compaction));
    readings.push(("compaction_without_index", without_index.compaction));
    let compacted = with_index.compacted;
    if let Some(compacted) = compacted.as_ref() {
        readings.push(("compacted_scan", compacted.scan));
        readings.push(("compacted_scan_again", compacted.scan_again));
        readings.push(("compacted_first", compacted.first));
        readings.push(("compacted_indexed", compacted.indexed));
    }

    // **And the shape**, measured last, so the probe's own map does not sit
    // in the memory the measurements were taken in.
    let shapes = file_shapes(&files);
    let map = key_map(&shapes);
    let compacted_rows = compacted.map_or(0, |compacted| compacted.rows);
    Iteration { readings, shapes, map, compacted_rows }
}

/// The shape's counts: the key's order, the distinct keys, and the files each
/// of the maps would name.
struct ShapeCounts {
    /// How many times the key falls over the table's rows.
    descents: usize,
    /// How many distinct keys the table holds.
    distinct: usize,
    /// How many rows repeat a key.
    duplicates: usize,
    /// The reach of the files' ranges.
    smallest_minimum: i64,
    /// The reach of the files' ranges.
    largest_maximum: i64,
    /// The range map's mean.
    range_mean: f64,
    /// The range map's smallest.
    range_smallest: usize,
    /// The range map's largest.
    range_largest: usize,
    /// The per-key map's mean.
    key_mean: f64,
    /// The per-key map's smallest.
    key_smallest: usize,
    /// The per-key map's largest.
    key_largest: usize,
    /// How many keys the maps were asked about.
    samples: usize,
}

impl ShapeCounts {
    /// The counts of the shape, over the table the database wrote.
    fn of(
        shapes: &[FileShape],
        map: &HashMap<i64, Vec<usize>>,
        table_keys: &[i64],
        sample: &[i64],
    ) -> Self {
        let range_counts: Vec<usize> =
            sample.iter().map(|key| range_map_files(shapes, *key)).collect();
        let key_counts: Vec<usize> = sample.iter().map(|key| key_map_files(map, *key)).collect();
        let (range_smallest, range_largest) = smallest_and_largest(&range_counts);
        let (key_smallest, key_largest) = smallest_and_largest(&key_counts);
        let distinct = table_keys.iter().copied().collect::<HashSet<_>>().len();
        let (smallest_minimum, largest_maximum) = range_extremes(shapes);
        Self {
            descents: descents(table_keys),
            distinct,
            duplicates: table_keys.len().saturating_sub(distinct),
            smallest_minimum,
            largest_maximum,
            range_mean: mean(&range_counts),
            range_smallest,
            range_largest,
            key_mean: mean(&key_counts),
            key_smallest,
            key_largest,
            samples: sample.len(),
        }
    }
}

/// Prints the shape: the key's order, the files' ranges, and the files each
/// of the maps would name.
fn print_shape(shapes: &[FileShape], counts: &ShapeCounts) {
    println!("\n--- the key's order ---");
    println!("descents: {}", counts.descents);
    println!("distinct keys: {}", counts.distinct);

    println!("\n--- each file's own range ---");
    println!("the range map's reach: {}..{}", counts.smallest_minimum, counts.largest_maximum);
    for (index, shape) in shapes.iter().enumerate() {
        println!("file {index:>3}: rows {:>7}, key {}..{}", shape.rows, shape.min, shape.max);
    }

    println!("\n--- the files each map names, over {} keys ---", counts.samples);
    println!(
        "the range map:   mean {:.3}, smallest {}, largest {}",
        counts.range_mean, counts.range_smallest, counts.range_largest
    );
    println!(
        "the per-key map: mean {:.3}, smallest {}, largest {}",
        counts.key_mean, counts.key_smallest, counts.key_largest
    );
}

/// The report the run publishes.
#[allow(clippy::too_many_arguments)]
fn report_of(
    arguments: &Arguments,
    rows: usize,
    lookup_row: usize,
    lookup_key: i64,
    compacted_rows: u64,
    shapes: &[FileShape],
    counts: &ShapeCounts,
    medians: &serde_json::Map<String, serde_json::Value>,
    spreads: &serde_json::Map<String, serde_json::Value>,
    derived: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "probe": "index",
        "data": arguments.data.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "rows": rows,
        "files": shapes.len(),
        "grid_rows": GRID,
        "key_column": KEY,
        "lookup_row": lookup_row,
        "lookup_key": lookup_key,
        "key_descents": counts.descents,
        "distinct_keys": counts.distinct,
        "duplicate_key_rows": counts.duplicates,
        "index_entries": counts.distinct,
        "compacted_rows": compacted_rows,
        "samples": counts.samples,
        "range_map_smallest_minimum": counts.smallest_minimum,
        "range_map_largest_maximum": counts.largest_maximum,
        "range_map_files_named_mean": counts.range_mean,
        "range_map_files_named_smallest": counts.range_smallest,
        "range_map_files_named_largest": counts.range_largest,
        "key_map_files_named_mean": counts.key_mean,
        "key_map_files_named_smallest": counts.key_smallest,
        "key_map_files_named_largest": counts.key_largest,
        "file_ranges": shapes
            .iter()
            .map(|shape| serde_json::json!({
                "rows": shape.rows,
                "min": shape.min,
                "max": shape.max,
            }))
            .collect::<Vec<_>>(),
        "measurements_ms": medians,
        "spreads_ms": spreads,
        "derived": derived,
    })
}

fn main() {
    let arguments = arguments();
    println!("data: {}", arguments.data.display());
    println!("grid: {GRID} rows, rows: {}", arguments.rows);

    let source = read_source(&arguments.data);
    // **And the table's rows leave a grid's worth behind**, because the
    // probe writes one grid past the table, to measure the write's cost.
    let rows = arguments.rows.min(source.keys.len().saturating_sub(GRID));
    println!("--- the source's own rows, read once ---");
    println!("rows: {}\n", source.keys.len());

    // **The lookup's key**: the row in the middle, as the database's own probe
    // takes it, so the two probes ask about the same lookup.
    let lookup_row = rows / 2;
    let lookup_key = source.keys[lookup_row];

    let mut iterations: Vec<Vec<Reading>> = Vec::with_capacity(ITERATIONS);
    let mut shapes: Vec<FileShape> = Vec::new();
    let mut map: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut compacted_rows = 0;
    for round in 0..ITERATIONS {
        let iteration = measure_once(&arguments, &source, rows, lookup_key, round % 2 == 1);
        let readings = iteration.readings;
        shapes = iteration.shapes;
        map = iteration.map;
        compacted_rows = iteration.compacted_rows;
        let took = |name: &str| {
            readings.iter().find(|(other, _)| *other == name).map_or(0.0, |(_, value)| ms(*value))
        };
        println!(
            "iteration {}: scan {:.3} ms, floor {:.3} ms, first {:.3} ms, indexed {:.3} ms, \
             write {:.3}/{:.3} ms, compaction {:.3}/{:.3} ms",
            round + 1,
            took("lookup_scan"),
            took("lookup_one_file"),
            took("lookup_first"),
            took("lookup_indexed"),
            took("write_no_index"),
            took("write_with_index"),
            took("compaction_without_index"),
            took("compaction_with_index"),
        );
        iterations.push(readings);
    }

    let table_keys: Vec<i64> = shapes.iter().flat_map(|shape| shape.keys.iter().copied()).collect();
    let sample = sample_keys(&table_keys, SAMPLES);
    let counts = ShapeCounts::of(&shapes, &map, &table_keys, &sample);
    print_shape(&shapes, &counts);

    let (medians, spreads) = timings_of(&iterations);
    let derived = derived_of(&medians);

    println!("\n--- the index's price ---");
    for name in [
        "write_no_index",
        "write_with_index",
        "lookup_scan",
        "lookup_one_file",
        "lookup_first",
        "lookup_indexed",
        "compaction_without_index",
        "compaction_with_index",
        "compacted_scan",
        "compacted_first",
        "compacted_indexed",
    ] {
        let median = medians.get(name).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        println!("{name:<24}: {median:>12.3} ms");
    }
    let figure = |name: &str| derived[name].as_f64().unwrap_or(0.0);
    println!("the index's build:  {:>12.3} ms", figure("lookup_build_ms"));
    println!("the saving:         {:>12.3} ms", figure("lookup_saving_ms"));
    println!("the break-even:     {:>12.3} lookups", figure("lookup_break_even"));
    println!("the write's tax:    {:>12.3} ms", figure("write_tax_ms"));
    println!("the compaction's tax:{:>12.3} ms", figure("compaction_tax_ms"));
    println!("the compacted build:{:>12.3} ms", figure("compacted_build_ms"));
    println!("the compacted saving:{:>12.3} ms", figure("compacted_saving_ms"));
    println!("the compacted break-even: {:>9.3} lookups", figure("compacted_break_even"));

    let report = report_of(
        &arguments,
        rows,
        lookup_row,
        lookup_key,
        compacted_rows,
        &shapes,
        &counts,
        &medians,
        &spreads,
        &derived,
    );
    println!("\n--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&arguments.work_dir).ok();
}
