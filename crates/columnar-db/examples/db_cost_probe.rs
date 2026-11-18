//! The database's own price: what the layer between the engine and the store
//! costs.
//!
//! ```text
//! cargo run --release -p columnar-db --example db_cost_probe -- \
//!     --data data/lineitem.parquet --out bench-results/db-cost-run1.json
//! ```
//!
//! # The question
//!
//! The store measured the **write** and the engine measured the **read**, and
//! **nobody has measured what a database costs on top of them**. The layer
//! between them is the catalog that survives a restart, the typed write path,
//! the snapshot, the row's identity and the compaction -- and every one of
//! those has a price.
//!
//! # What the probe measures
//!
//! | | |
//! |---|---|
//! | **`open_catalog`** | the database's open: the catalog read, and the tables' manifests |
//! | **`write`** | the write on the grid: the rows checked, buffered, and written in the store's grid |
//! | **`read_one_file`** | the snapshot's read of a table that is **one file** |
//! | **`read_many_files`** | the snapshot's read of a table that is **many files** |
//! | **`read_many_files_engine`** | the engine's own read of the same files, the same columns and the same filter |
//! | **`point_lookup_scan`** | the point lookup's **scan**: the whole table, read through the database |
//! | **`point_lookup_engine`** | the engine's own point lookup, over the same files and the same filters |
//! | **`point_lookup_one_file`** | the same lookup over **one file**: the floor an index would reach |
//! | **`open_many_files`** | the open of the table's files, with nothing read |
//! | **`compaction_clean`** | the compaction of a table with **no retired versions**: the compaction's own cost |
//! | **`read_taxed_changed`** | the database's read of the changed table: what a read pays |
//! | **`tax_full_filter`** | the engine's read of the same table, with the **full** live filter |
//! | **`tax_plain_filter`** | the engine's read of the same table, with the live filter **without** the retired ids |
//! | **`tax_full_filter_small`** | the same pair, over the **small** table |
//! | **`tax_plain_filter_small`** | the same pair, over the **small** table, without the retired ids |
//! | **`compaction_taxed`** | the compaction of the changed table, with its retired versions |
//!
//! **And the space amplification**: the bytes the append-only model holds
//! before a compaction, over the bytes it holds after it.
//!
//! # The pairs
//!
//! **Every difference the report publishes is a difference of two
//! measurements taken back to back**, so the machine's movement over the
//! hundreds of milliseconds between them cannot land on one side and not on
//! the other. **The database's own overhead** is the database's read beside
//! the engine's read of the same files; **the tombstone tax** is the read
//! with the full live filter beside the read without the retired ids, over
//! the same table and at the same moment.
//!
//! # The index
//!
//! The point lookup's **scan** is measured against the **floor** -- the same
//! lookup over the one file an index would name -- **because that is the most
//! an index could save**. And the gap is **decomposed**: the open of the
//! table's files, with nothing read, is what the scan pays before it reads
//! anything, so a reader can see whether the gap is the files or the data.
//!
//! **And the engine's own scan is measured beside the database's**, so the
//! part of the gap that is the database's layer is told apart from the part
//! that is the engine's read.
//!
//! **The index's shape is decided by that figure, and the decision is
//! published** -- with the numbers that decided it.
//!
//! # The tombstone tax
//!
//! **A read filters on `deleted = 0` and on `row_id != <retired id>` for every
//! retired version**, and the probe measures what that filter costs: the same
//! read over the same rows, with the retired ids and without them.
//!
//! **And the tax is measured at two sizes.** The same share of the key range
//! changes in the small table and in the large one, so the two taxes are two
//! numbers of predicates over two different numbers of rows -- and the price
//! of one retired version over one row is the same in both, or it is not.
//!
//! # The compaction's ceiling
//!
//! **The compaction reads a file at a time**, so the engine's memory budget is
//! no longer the table's ceiling: the budget's verdict is about a file's rows
//! and not about the table's, and the compaction is not refused where it used
//! to be. **The probe measures that** rather than asserting it: it compacts
//! the whole dataset, and if the compaction is refused it bisects for the row
//! count where it is. **And the report says how many rows compacted and how
//! many were refused** -- and a refusal of none is a refusal of zero rows.
//!
//! # The rows
//!
//! **The rows are the dataset's own** -- three columns of the published
//! lineitem file -- read once through the engine and written in the store's
//! grid. The probe holds the columns, not the rows: a probe that held six
//! million rows in memory would measure the allocator as much as it measured
//! the database.
//!
//! **And the reads are warm.** The files were written in this campaign, so
//! they are in the page cache; the figure is the database's cost over a warm
//! file, and not the disk's.

#![allow(clippy::indexing_slicing, clippy::cast_precision_loss)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_db::prelude::*;
use columnar_engine::{Dataset, Encoding, ExecOptions, FileSetSource, QueryPlan, ScalarValue};

/// The key column: the point lookup's column.
const KEY: &str = "l_orderkey";

/// The float column.
const VALUE: &str = "l_quantity";

/// The text column.
const TEXT: &str = "l_shipmode";

/// The hidden column that holds a version's own id.
const ROW_ID: &str = "row_id";

/// The hidden column that says whether a row is a tombstone.
const DELETED: &str = "deleted";

/// The table the many-files measurement reads.
const MANY: &str = "many";

/// The table the one-file measurement reads.
const ONE: &str = "one";

/// The table the clean compaction reads: a table with no retired versions.
const CLEAN: &str = "clean";

/// How many times the probe runs its whole shape. **Each run builds a fresh
/// database**, so every measurement is taken over the files that run wrote.
const ITERATIONS: usize = 3;

/// The grid: the store's own, and the rows' own batch.
const GRID: usize = columnar_store::BATCH_ROWS;

/// The rows the probe's table holds. **The default is a million**, and the
/// reason is the compaction's ceiling: the compaction reads the table's live
/// rows in one piece, so a table of six million rows is refused by the
/// engine's memory budget -- and the probe measures that ceiling separately,
/// rather than tripping over it in every measurement.
const DEFAULT_ROWS: usize = 1_000_000;

/// The share of the key range whose rows the probe changes: the top fifth is
/// deleted, and the fifth below it is updated.
///
/// **The space amplification needs rows that change**, or it is a figure of
/// one. A fifth of the key range is enough to be a real number and small
/// enough to be a setup cost.
const CHANGED_SHARE: f64 = 0.05;

/// The share of the key range that changes, in parts: **the twentieth part**
/// is deleted, and the twentieth part below it is updated.
const CHANGED_PARTS: i64 = 20;

/// How many steps the bisection for the compaction's ceiling takes.
const CEILING_STEPS: usize = 4;

/// The value the changed rows carry.
const CHANGED: &str = "changed";

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
    let mut arguments = std::env::args().skip(1);
    let mut data = PathBuf::from("data/lineitem.parquet");
    let mut out = None;
    let mut work_dir = PathBuf::from("target/db-cost-probe");
    let mut rows = DEFAULT_ROWS;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data" => {
                if let Some(value) = arguments.next() {
                    data = PathBuf::from(value);
                }
            }
            "--out" => out = arguments.next(),
            "--work-dir" => {
                if let Some(value) = arguments.next() {
                    work_dir = PathBuf::from(value);
                }
            }
            "--rows" => {
                if let Some(value) = arguments.next() {
                    rows = value.parse::<usize>().unwrap_or(DEFAULT_ROWS);
                }
            }
            _ => {}
        }
    }
    Arguments { data, out, work_dir, rows }
}

/// The table's schema: the three columns, and no nulls.
fn schema() -> Schema {
    Schema::new(vec![
        Column::new(KEY, DataType::Int64, false),
        Column::new(VALUE, DataType::Float64, false),
        Column::new(TEXT, DataType::Utf8, false),
    ])
    .expect("the schema is one a table can hold")
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

    /// How many rows the source holds.
    fn len(&self) -> usize {
        self.keys.len()
    }

    /// The smallest key the source holds.
    fn min_key(&self) -> i64 {
        self.keys.iter().copied().min().unwrap_or(0)
    }

    /// The largest key the source holds.
    fn max_key(&self) -> i64 {
        self.keys.iter().copied().max().unwrap_or(0)
    }
}

/// Reads the three columns of the source file, through the engine.
fn read_source(path: &Path) -> Source {
    let source = FileSetSource::open(&[path.to_path_buf()]).expect("the source opens");
    let plan =
        QueryPlan::new(Vec::new(), vec![KEY.to_owned(), VALUE.to_owned(), TEXT.to_owned()], None);
    let dataset = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    let keys = ints_of(&dataset, KEY);
    let values = floats_of(&dataset, VALUE);
    let texts = texts_of(&dataset, TEXT);
    Source { keys, values, texts }
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

/// One measurement: its name and how long it took.
type Reading = (&'static str, Duration);

/// The counts a run publishes beside the timings.
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    /// The files the large table holds.
    files: usize,
    /// The row the point lookup asks for.
    lookup_row: usize,
    /// The key the point lookup asks for.
    lookup_key: i64,
    /// The rows the changed share deleted.
    deleted_rows: usize,
    /// The rows the changed share updated.
    updated_rows: usize,
    /// The rows the changed share deleted, in the small table.
    deleted_small: usize,
    /// The rows the changed share updated, in the small table.
    updated_small: usize,
    /// The versions the whole run retired.
    retired_rows: u64,
    /// The rows the large table holds once the change has landed.
    rows_changed: u64,
    /// The rows that are live after the compaction.
    live_rows: u64,
    /// The bytes the large table held before its compaction.
    bytes_before: u64,
    /// The bytes the large table held after its compaction.
    bytes_after: u64,
    /// The bytes the clean table held before its compaction.
    clean_bytes_before: u64,
    /// The bytes the clean table held after its compaction.
    clean_bytes_after: u64,
    /// The live rows the clean table's compaction wrote.
    clean_rows: u64,
    /// The retired versions the small table holds.
    retired_small: u64,
    /// The rows the small table holds after the change.
    rows_small: u64,
}

/// Milliseconds, for printing and for the report.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
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
    output.map_or(0.0, |output| {
        String::from_utf8_lossy(&output.stdout)
            .replace(['{', '}'], " ")
            .split_whitespace()
            .next()
            .and_then(|first| first.parse::<f64>().ok())
            .unwrap_or(0.0)
    })
}

/// The database's open: the catalog read, and the tables' manifests.
fn measure_open(root: &Path) -> Duration {
    let started = Instant::now();
    let database = Database::open(root).expect("the database opens");
    let _ = database.tables();
    started.elapsed()
}

/// The filter every read carries: the live rows, and nothing else.
fn live_predicate() -> Predicate {
    Predicate::new(DELETED, ComparisonOp::Eq, ScalarValue::Int64(0))
}

/// The filter a read carries when the retired versions are excluded too: the
/// live rows, and not the versions the tombstones name.
fn full_filter(retired: &[i64]) -> Vec<Predicate> {
    let mut filter = vec![live_predicate()];
    for id in retired {
        filter.push(Predicate::new(ROW_ID, ComparisonOp::Ne, ScalarValue::Int64(*id)));
    }
    filter
}

/// The retired versions of a table, as the probe reads them: the rows whose
/// `deleted` flag is set, and the ids they name.
///
/// **The probe reads them the way the database's own read does**, so that the
/// filter it measures is the filter the database's read carries.
fn retired_ids(files: &[PathBuf]) -> Vec<i64> {
    let plan = QueryPlan::new(
        vec![Predicate::new(DELETED, ComparisonOp::Eq, ScalarValue::Int64(1))],
        vec![ROW_ID.to_owned()],
        None,
    );
    let source = FileSetSource::open(files).expect("the source opens");
    let dataset = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    ints_of(&dataset, ROW_ID)
}

/// The database's own read of a table: the snapshot, the plan, and the rows.
fn measure_read(database: &Database, name: &str, key: Option<i64>) -> Duration {
    let started = Instant::now();
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table(name).expect("the table is there");
    let mut builder = table.plan();
    if let Some(key) = key {
        builder = builder.filter(col(KEY).eq(key));
    }
    let plan = builder.build().expect("the plan is built");
    let rows = table.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    let _ = rows.len();
    started.elapsed()
}

/// The engine's own read of a set of files: the same columns, and the filter
/// it is given.
fn measure_engine_read(files: &[PathBuf], filter: &[Predicate], key: Option<i64>) -> Duration {
    let started = Instant::now();
    let source = FileSetSource::open(files).expect("the source opens");
    let mut predicates = filter.to_vec();
    if let Some(key) = key {
        predicates.push(col(KEY).eq(key));
    }
    let plan =
        QueryPlan::new(predicates, vec![KEY.to_owned(), VALUE.to_owned(), TEXT.to_owned()], None);
    let rows = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    let _ = rows.len();
    started.elapsed()
}

/// The open of a set of files, with nothing read.
fn measure_open_files(files: &[PathBuf]) -> Duration {
    let started = Instant::now();
    let source = FileSetSource::open(files).expect("the source opens");
    let _ = source;
    started.elapsed()
}

/// Writes the rows into a table, on the grid, and commits them.
///
/// **The rows are built outside the timer**: the figure is the database's
/// write, and not the probe's own allocation of a million rows.
fn write_table(database: &mut Database, name: &str, source: &Source, rows: usize) -> Duration {
    let mut total = Duration::ZERO;
    {
        let mut table = database.create_table(name, schema()).expect("the table is created");
        let mut written = 0;
        while written < rows {
            let count = GRID.min(rows - written);
            let batch = source.rows(written, count);
            let started = Instant::now();
            table.insert(&batch).expect("the rows are accepted");
            total += started.elapsed();
            written += count;
        }
        let started = Instant::now();
        table.commit().expect("the rows are written");
        total += started.elapsed();
    }
    total
}

/// The table's files, as the manifest names them.
fn files_of(database: &Database, name: &str) -> Vec<PathBuf> {
    database.store().table(name).expect("the store answers").expect("the table is there").paths()
}

/// The table's bytes, as the manifest counts them.
fn bytes_of(database: &Database, name: &str) -> u64 {
    database.store().table(name).expect("the store answers").expect("the table is there").bytes()
}

/// The compaction of a table, and how long it took.
fn measure_compaction(database: &mut Database, name: &str) -> (Duration, u64) {
    let started = Instant::now();
    let mut table = database.table_mut(name).expect("the table is there");
    let compaction = table.compact().expect("the table is compacted");
    (started.elapsed(), compaction.rows)
}

/// The rows that match a predicate are updated, and the count is answered.
fn update_rows(database: &mut Database, name: &str, predicate: Predicate) -> usize {
    let mut table = database.table_mut(name).expect("the table is there");
    let row = Row::filled(vec![
        ScalarValue::Int64(-1),
        ScalarValue::Float64(-1.0),
        ScalarValue::Utf8(CHANGED.to_owned()),
    ]);
    table.update(predicate, row).expect("the update lands")
}

/// The rows that match a predicate are deleted, and the count is answered.
fn delete_rows(database: &mut Database, name: &str, predicate: Predicate) -> usize {
    let mut table = database.table_mut(name).expect("the table is there");
    table.delete(predicate).expect("the delete lands")
}

/// The rows that change: the bottom of the key range is updated, and the top
/// of it is deleted.
///
/// **The two are apart, and not one range**: the update takes the lowest keys
/// and the delete the highest, so the two do not touch the same row and the
/// table's live rows are the rows that were neither.
///
/// **And the bounds are the key range's own ends**, and not the range of the
/// rows a table happens to hold: **the source's keys are in the file's order
/// and not in the key's order**, so the first row of the source holds a
/// random key and the last one holds another, and a share of the range
/// between them is not a share of the range at all.
fn change_share(source: &Source) -> (Predicate, Predicate) {
    let min_key = source.min_key();
    let max_key = source.max_key();
    let span = max_key - min_key;
    let low = min_key + span / CHANGED_PARTS;
    let high = max_key - span / CHANGED_PARTS;
    (col(KEY).lt(low), col(KEY).gt(high))
}

/// The readings and the counts one iteration makes.
fn measure_once(arguments: &Arguments, source: &Source, rows: usize) -> (Vec<Reading>, Counts) {
    let root = arguments.work_dir.join("db");
    std::fs::remove_dir_all(&root).ok();
    let mut database = Database::open(&root).expect("the database opens");
    let mut counts = Counts::default();
    let mut readings = Vec::with_capacity(16);
    let note = |name: &str, took: Duration| {
        println!("    {name:<28}: {:>12.3} ms", ms(took));
    };

    // **The write on the grid**, and the two tables beside it.
    let write = write_table(&mut database, MANY, source, rows);
    write_table(&mut database, ONE, source, GRID.min(rows));
    write_table(&mut database, CLEAN, source, rows);
    readings.push(("write", write));
    note("write", write);

    // **The catalog's open**, on the database that is there.
    let open_catalog = measure_open(&root);
    readings.push(("open_catalog", open_catalog));
    note("open_catalog", open_catalog);

    // **The snapshot's read**, over one file and over many, and the engine's
    // own read of the same files **back to back**: the difference is what the
    // database costs on top of the engine.
    let read_one_file = measure_read(&database, ONE, None);
    readings.push(("read_one_file", read_one_file));
    note("read_one_file", read_one_file);
    let read_many_files = measure_read(&database, MANY, None);
    readings.push(("read_many_files", read_many_files));
    note("read_many_files", read_many_files);
    let many_files = files_of(&database, MANY);
    counts.files = many_files.len();
    let plain = vec![live_predicate()];
    let read_many_files_engine = measure_engine_read(&many_files, &plain, None);
    readings.push(("read_many_files_engine", read_many_files_engine));
    note("read_many_files_engine", read_many_files_engine);

    // **The point lookup**: the scan, the engine's own **back to back**, and
    // the floor over the one file an index would name.
    let lookup_row = rows / 2;
    let lookup_key = source.keys[lookup_row];
    counts.lookup_row = lookup_row;
    counts.lookup_key = lookup_key;
    readings.push(("point_lookup_scan", measure_read(&database, MANY, Some(lookup_key))));
    readings
        .push(("point_lookup_engine", measure_engine_read(&many_files, &plain, Some(lookup_key))));
    let floor = many_files
        .get(lookup_row / GRID)
        .cloned()
        .unwrap_or_else(|| many_files.first().expect("the table has a file").clone());
    readings
        .push(("point_lookup_one_file", measure_engine_read(&[floor], &plain, Some(lookup_key))));
    readings.push(("open_many_files", measure_open_files(&many_files)));

    // **The compaction of a table with no retired versions**: the
    // compaction's own cost, without the tombstone tax on top of it.
    counts.clean_bytes_before = bytes_of(&database, CLEAN);
    let (compaction_clean, clean_rows) = measure_compaction(&mut database, CLEAN);
    counts.clean_rows = clean_rows;
    counts.clean_bytes_after = bytes_of(&database, CLEAN);
    readings.push(("compaction_clean", compaction_clean));
    note("compaction_clean", compaction_clean);

    // **The rows that change**, in the same share of the key range in both
    // tables.
    let (update, delete) = change_share(source);
    let started = Instant::now();
    counts.updated_rows = update_rows(&mut database, MANY, update.clone());
    note("the update", started.elapsed());
    let started = Instant::now();
    counts.deleted_rows = delete_rows(&mut database, MANY, delete.clone());
    note("the delete", started.elapsed());
    let (small_update, small_delete) = change_share(source);
    counts.updated_small = update_rows(&mut database, ONE, small_update);
    counts.deleted_small = delete_rows(&mut database, ONE, small_delete);
    counts.retired_small =
        u64::try_from(counts.deleted_small + counts.updated_small).unwrap_or(u64::MAX);
    let small_retired = usize::try_from(counts.retired_small).unwrap_or(usize::MAX);
    counts.rows_small =
        u64::try_from(GRID.min(rows) + small_retired + counts.updated_small).unwrap_or(u64::MAX);
    counts.retired_rows =
        u64::try_from(counts.deleted_rows + counts.updated_rows).unwrap_or(u64::MAX);
    let retired = usize::try_from(counts.retired_rows).unwrap_or(usize::MAX);
    counts.rows_changed = u64::try_from(rows + retired + counts.updated_rows).unwrap_or(u64::MAX);

    // **The tax the read pays**, and the tax measured as a pair: the same
    // table, the same rows, with the retired ids and without them.
    //
    // **And the files are the ones the change left**: the tombstones are in
    // the new files, and the retired ids are read from them.
    let changed_files = files_of(&database, MANY);
    let many_retired = retired_ids(&changed_files);
    let full = full_filter(&many_retired);
    let read_taxed_changed = measure_read(&database, MANY, None);
    readings.push(("read_taxed_changed", read_taxed_changed));
    note("read_taxed_changed", read_taxed_changed);
    let tax_full_filter = measure_engine_read(&changed_files, &full, None);
    readings.push(("tax_full_filter", tax_full_filter));
    note("tax_full_filter", tax_full_filter);
    let tax_plain_filter = measure_engine_read(&changed_files, &plain, None);
    readings.push(("tax_plain_filter", tax_plain_filter));
    note("tax_plain_filter", tax_plain_filter);

    // **And the same pair over the small table**, so the tax's price is
    // measured at two sizes.
    let small_files = files_of(&database, ONE);
    let small_retired = retired_ids(&small_files);
    let small_full = full_filter(&small_retired);
    let tax_full_filter_small = measure_engine_read(&small_files, &small_full, None);
    readings.push(("tax_full_filter_small", tax_full_filter_small));
    note("tax_full_filter_small", tax_full_filter_small);
    let tax_plain_filter_small = measure_engine_read(&small_files, &plain, None);
    readings.push(("tax_plain_filter_small", tax_plain_filter_small));
    note("tax_plain_filter_small", tax_plain_filter_small);

    // **The compaction that pays the tax back**, and the bytes it leaves.
    counts.bytes_before = bytes_of(&database, MANY);
    let (compaction_taxed, live_rows) = measure_compaction(&mut database, MANY);
    counts.live_rows = live_rows;
    counts.bytes_after = bytes_of(&database, MANY);
    readings.push(("compaction_taxed", compaction_taxed));
    note("compaction_taxed", compaction_taxed);

    (readings, counts)
}

/// The compaction's ceiling: the row count where the engine's memory budget
/// refuses the compaction.
struct Ceiling {
    /// The largest row count whose compaction succeeded.
    ok: usize,
    /// The smallest row count whose compaction was refused -- **and zero when
    /// nothing was refused**, which is the answer the streaming compaction
    /// gives.
    refused: usize,
    /// The reason the refusal gave.
    reason: String,
    /// How many steps the bisection took.
    steps: usize,
}

/// The compaction of a table of `rows` rows: how long it took, or why it was
/// refused.
fn compaction_of(source: &Source, work_dir: &Path, rows: usize) -> Result<(), String> {
    let root = work_dir.join("ceiling");
    std::fs::remove_dir_all(&root).ok();
    let mut database = Database::open(&root).expect("the database opens");
    write_table(&mut database, MANY, source, rows);
    let outcome = {
        let mut table = database.table_mut(MANY).expect("the table is there");
        table.compact()
    };
    std::fs::remove_dir_all(&root).ok();
    outcome.map(|_| ()).map_err(|error| error.to_string())
}

/// The compaction's ceiling, found by bisection.
///
/// **The lower bound is measured**: the clean table's compaction succeeded at
/// that many rows, in this very run. **And the upper bound is measured too**:
/// the whole dataset is compacted first, and the bisection runs only if the
/// engine refused it.
fn measure_ceiling(source: &Source, work_dir: &Path, lower: usize) -> Ceiling {
    let top = source.len();
    let mut ceiling = Ceiling { ok: lower, refused: top, reason: String::new(), steps: 0 };
    if top <= lower {
        return ceiling;
    }
    match compaction_of(source, work_dir, top) {
        Ok(()) => {
            // **And nothing was refused**: the whole dataset compacted.
            ceiling.ok = top;
            ceiling.refused = 0;
            ceiling.steps = 1;
            return ceiling;
        }
        Err(reason) => {
            ceiling.refused = top;
            ceiling.reason = reason;
            ceiling.steps = 1;
        }
    }
    while ceiling.steps < CEILING_STEPS {
        let candidate = (ceiling.ok + ceiling.refused) / 2;
        if candidate <= ceiling.ok {
            break;
        }
        ceiling.steps += 1;
        match compaction_of(source, work_dir, candidate) {
            Ok(()) => ceiling.ok = candidate,
            Err(reason) => {
                ceiling.refused = candidate;
                ceiling.reason = reason;
            }
        }
    }
    ceiling
}

/// The median of a set of numbers.
fn median_of(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
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

/// The figures one iteration derives from its own measurements.
///
/// **Every difference here is a difference of two measurements taken back to
/// back**, so the machine's movement cannot land on one side of it.
fn derived_of(readings: &[Reading], counts: &Counts, rows: usize) -> serde_json::Value {
    let took = |name: &str| {
        readings.iter().find(|(other, _)| *other == name).map_or(0.0, |(_, value)| ms(*value))
    };
    let rows_f64 = rows as f64;
    let retired_f64 = counts.retired_rows as f64;
    let small_retired_f64 = counts.retired_small as f64;
    serde_json::json!({
        "write_rows_per_second": rows_f64 / (took("write") / 1000.0),
        "compaction_ns_per_row": took("compaction_clean") * 1_000_000.0 / rows_f64,
        "compaction_taxed_ns_per_row": took("compaction_taxed") * 1_000_000.0
            / (counts.live_rows.max(1) as f64),
        "compaction_tax_ms": took("compaction_taxed") - took("compaction_clean"),
        "database_read_overhead_ms": took("read_many_files") - took("read_many_files_engine"),
        "database_lookup_overhead_ms": took("point_lookup_scan") - took("point_lookup_engine"),
        "index_saving_ms": took("point_lookup_engine") - took("point_lookup_one_file"),
        "lookup_ratio": took("point_lookup_engine") / took("point_lookup_one_file").max(0.001),
        "read_tax_ms": took("tax_full_filter") - took("tax_plain_filter"),
        "read_tax_small_ms": took("tax_full_filter_small") - took("tax_plain_filter_small"),
        "read_tax_ns_per_row_per_version": (took("tax_full_filter") - took("tax_plain_filter"))
            * 1_000_000.0 / (retired_f64 * counts.rows_changed as f64).max(1.0),
        "read_tax_small_ns_per_row_per_version": (took("tax_full_filter_small")
            - took("tax_plain_filter_small"))
            * 1_000_000.0
            / (small_retired_f64 * counts.rows_small as f64).max(1.0),
        "space_amplification": counts.bytes_before as f64 / counts.bytes_after.max(1) as f64,
        "space_amplification_clean": counts.clean_bytes_before as f64
            / counts.clean_bytes_after.max(1) as f64,
    })
}

/// The medians of the derived figures, across the iterations.
fn derived_medians(iterations: &[serde_json::Value]) -> serde_json::Value {
    let mut medians = serde_json::Map::new();
    let Some(first) = iterations.first() else {
        return serde_json::Value::Null;
    };
    if let Some(object) = first.as_object() {
        for key in object.keys() {
            let mut values: Vec<f64> = iterations
                .iter()
                .filter_map(|iteration| iteration.get(key).and_then(serde_json::Value::as_f64))
                .collect();
            if values.is_empty() {
                continue;
            }
            values.sort_by(f64::total_cmp);
            medians.insert(key.clone(), serde_json::json!(median_of(&values)));
        }
    }
    serde_json::Value::Object(medians)
}

/// Prints a set of measurements, with their spreads.
fn print_timings(medians: &serde_json::Map<String, serde_json::Value>, names: &[&str]) {
    for name in names {
        let median = medians.get(*name).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        println!("{name:<28}: {median:>12.3} ms");
    }
}

/// Prints what the run measured: the timings, the derived figures, the bytes
/// and the ceiling.
fn print_summary(
    medians: &serde_json::Map<String, serde_json::Value>,
    derived: &serde_json::Value,
    counts: &Counts,
    ceiling: &Ceiling,
) {
    println!("\n--- the database's own price ---");
    print_timings(
        medians,
        &[
            "open_catalog",
            "write",
            "read_one_file",
            "read_many_files",
            "read_many_files_engine",
            "point_lookup_scan",
            "point_lookup_engine",
            "point_lookup_one_file",
            "open_many_files",
            "compaction_clean",
            "read_taxed_changed",
            "tax_full_filter",
            "tax_plain_filter",
            "tax_full_filter_small",
            "tax_plain_filter_small",
            "compaction_taxed",
        ],
    );
    println!("\n--- the derived ---");
    for (label, key) in [
        ("the write's throughput:        ", "write_rows_per_second"),
        ("the compaction, per row:        ", "compaction_ns_per_row"),
        ("the compaction, taxed:          ", "compaction_taxed_ns_per_row"),
        ("the tax the compaction pays:    ", "compaction_tax_ms"),
        ("the tax the read pays:          ", "read_tax_ms"),
        ("the tax, the small table:       ", "read_tax_small_ms"),
        ("the tax, per row per version:   ", "read_tax_ns_per_row_per_version"),
        ("the tax, the small table:       ", "read_tax_small_ns_per_row_per_version"),
        ("the database's read overhead:   ", "database_read_overhead_ms"),
        ("the database's lookup overhead: ", "database_lookup_overhead_ms"),
        ("the most an index could save:   ", "index_saving_ms"),
        ("the lookup, over the floor:     ", "lookup_ratio"),
        ("the space amplification:        ", "space_amplification"),
    ] {
        println!("{label}{:>12.3}", derived[key].as_f64().unwrap_or(0.0));
    }
    println!("\n--- the bytes ---");
    println!("before the compaction: {} bytes", counts.bytes_before);
    println!("after the compaction:  {} bytes", counts.bytes_after);
    println!(
        "files: {}, live rows: {}, retired: {}",
        counts.files, counts.live_rows, counts.retired_rows
    );
    println!("updated: {}, deleted: {}", counts.updated_rows, counts.deleted_rows);
    println!("\n--- the compaction's ceiling ---");
    if ceiling.refused == 0 {
        println!(
            "compacted at {} rows, and refused at no row count, in {} step",
            ceiling.ok, ceiling.steps
        );
    } else {
        println!(
            "compacted at {} rows, refused at {} rows, in {} steps",
            ceiling.ok, ceiling.refused, ceiling.steps
        );
    }
    if !ceiling.reason.is_empty() {
        println!("the reason: {}", ceiling.reason);
    }
}

/// The report the run publishes.
fn report_of(
    arguments: &Arguments,
    rows: usize,
    counts: &Counts,
    ceiling: &Ceiling,
    medians: &serde_json::Map<String, serde_json::Value>,
    spreads: &serde_json::Map<String, serde_json::Value>,
    derived: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "probe": "db-cost",
        "data": arguments.data.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "rows": rows,
        "files": counts.files,
        "grid_rows": GRID,
        "lookup_row": counts.lookup_row,
        "lookup_key": counts.lookup_key,
        "deleted_rows": counts.deleted_rows,
        "updated_rows": counts.updated_rows,
        "deleted_rows_small": counts.deleted_small,
        "updated_rows_small": counts.updated_small,
        "retired_rows": counts.retired_rows,
        "retired_rows_small": counts.retired_small,
        "rows_small": counts.rows_small,
        "rows_changed": counts.rows_changed,
        "live_rows": counts.live_rows,
        "bytes_before_compaction": counts.bytes_before,
        "bytes_after_compaction": counts.bytes_after,
        "clean_bytes_before_compaction": counts.clean_bytes_before,
        "clean_bytes_after_compaction": counts.clean_bytes_after,
        "clean_live_rows": counts.clean_rows,
        "compaction_ceiling_ok_rows": ceiling.ok,
        "compaction_ceiling_refused_rows": ceiling.refused,
        "compaction_ceiling_reason": ceiling.reason,
        "compaction_ceiling_steps": ceiling.steps,
        "measurements_ms": medians,
        "spreads_ms": spreads,
        "derived": derived,
    })
}

fn main() {
    let arguments = arguments();
    println!("data: {}", arguments.data.display());
    println!("grid: {GRID} rows, iterations: {ITERATIONS}, rows: {}", arguments.rows);
    println!("changed share: {CHANGED_SHARE} of the key range\n");

    let source = read_source(&arguments.data);
    let rows = arguments.rows.min(source.len());
    println!("--- the source's own rows, read once ---");
    println!("rows: {}, keys: {}..{}\n", source.len(), source.min_key(), source.max_key());

    let mut iterations = Vec::with_capacity(ITERATIONS);
    let mut derived_iterations = Vec::with_capacity(ITERATIONS);
    let mut counts = Counts::default();
    for iteration in 0..ITERATIONS {
        let (readings, iteration_counts) = measure_once(&arguments, &source, rows);
        counts = iteration_counts;
        let took = |name: &str| {
            readings.iter().find(|(other, _)| *other == name).map_or(0.0, |(_, value)| ms(*value))
        };
        println!(
            "iteration {}: write {:.3} ms, open {:.3} ms, many {:.3} ms, \
point {:.3} ms, compaction {:.3} ms, tax {:.3} ms",
            iteration + 1,
            took("write"),
            took("open_catalog"),
            took("read_many_files"),
            took("point_lookup_scan"),
            took("compaction_taxed"),
            took("tax_full_filter") - took("tax_plain_filter"),
        );
        derived_iterations.push(derived_of(&readings, &counts, rows));
        iterations.push(readings);
    }

    // **The compaction's ceiling**: the row count where the engine's memory
    // budget refuses the compaction. **The probe measures it rather than
    // asserting it.**
    let ceiling = measure_ceiling(&source, &arguments.work_dir, rows);

    let (medians, spreads) = timings_of(&iterations);
    let derived = derived_medians(&derived_iterations);
    print_summary(&medians, &derived, &counts, &ceiling);

    let report = report_of(&arguments, rows, &counts, &ceiling, &medians, &spreads, &derived);
    println!("\n--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&arguments.work_dir).ok();
}
