//! The index's shape, measured before it is built.
//!
//! ```text
//! cargo run --release -p columnar-db --example index_probe -- \
//!     --data data/lineitem.parquet --out bench-results/index-run1.json
//! ```
//!
//! # The question
//!
//! **The index's shape is decided by a figure, and the index is not built.**
//! [ADR-0066](../../../docs/adr/0066-the-databases-own-price.md) gave the rule
//! -- *a point lookup that a scan already answers more cheaply is not given an
//! index, and the database says so* -- and the figure: the point lookup costs
//! **43.319 ms** as a scan and **2.657 ms** over the one file an index would
//! reach. **And the index's own price is unmeasured, because there is no index
//! to measure.**
//!
//! **And the price that is missing is the price that decides the shape.**
//! ADR-0066 chose *a map from the key to the file that holds it*, and it chose
//! it for the **granularity** -- the file, because that is where the floor was
//! measured -- and not for the **encoding**. **So the encoding is measured
//! here, before anything is built.**
//!
//! # The two maps
//!
//! | | |
//! |---|---|
//! | **the range map** | the key's range in each file: two numbers a file |
//! | **the per-key map** | an entry a key: the files that hold it |
//!
//! **And the range map has two homes**: the file's own footer, which the store
//! already writes (`EnabledStatistics::Chunk`) and which costs nothing to keep,
//! and the database's own memory, which a write pays for. **And the two name
//! the same files, because the file's own statistics are that range** -- so the
//! experiment answers the shape's question once, and the price decides which
//! home the range map takes.
//!
//! # What the probe measures
//!
//! | | |
//! |---|---|
//! | **the key's order** | whether the key rises in the table's rows, and how often it does not |
//! | **each file's range** | the key's minimum and maximum in each of the table's files |
//! | **the distinct keys** | how many entries the per-key map holds |
//! | **the files each map names** | over a sample of keys, how many files each map would read |
//!
//! **And the answer is not a guess**: a map that cannot narrow the files on
//! this data saves nothing, and a map that saves nothing is not built.
//!
//! # The table
//!
//! **The same table the database's own probe builds**: a million rows of the
//! published lineitem file -- three columns, written on the store's grid, so
//! **sixteen files of 65,536 rows** -- and the same key column, so the numbers
//! here and the numbers in [PERF.md §3ak](../../../PERF.md#3ak-the-databases-own-price)
//! are about the same table.

#![allow(clippy::indexing_slicing, clippy::cast_precision_loss)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

/// The table's schema: the three columns, and no nulls.
fn schema() -> Schema {
    Schema::new(vec![
        Column::new(KEY, DataType::Int64, false),
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

fn main() {
    let arguments = arguments();
    println!("data: {}", arguments.data.display());
    println!("grid: {GRID} rows, rows: {}", arguments.rows);

    let source = read_source(&arguments.data);
    let rows = arguments.rows.min(source.keys.len());
    println!("--- the source's own rows, read once ---");
    println!("rows: {}\n", source.keys.len());

    let mut database = Database::open(&arguments.work_dir).expect("the database opens");
    write_table(&mut database, TABLE, &source, rows);
    let files = files_of(&database, TABLE);
    println!("\n--- the table the database wrote ---");
    println!("rows: {rows}, files: {}", files.len());

    let shapes = file_shapes(&files);
    let map = key_map(&shapes);
    let table_keys: Vec<i64> = shapes.iter().flat_map(|shape| shape.keys.iter().copied()).collect();
    let distinct = table_keys.iter().copied().collect::<HashSet<_>>().len();

    println!("\n--- the key's order ---");
    println!("descents: {}", descents(&table_keys));
    println!("distinct keys: {distinct}");

    println!("\n--- each file's own range ---");
    let (smallest_minimum, largest_maximum) = range_extremes(&shapes);
    println!("the range map's reach: {smallest_minimum}..{largest_maximum}");
    for (index, shape) in shapes.iter().enumerate() {
        println!("file {index:>3}: rows {:>7}, key {}..{}", shape.rows, shape.min, shape.max);
    }

    let sample = sample_keys(&table_keys, SAMPLES);
    let range_counts: Vec<usize> =
        sample.iter().map(|key| range_map_files(&shapes, *key)).collect();
    let key_counts: Vec<usize> = sample.iter().map(|key| key_map_files(&map, *key)).collect();
    let (range_smallest, range_largest) = smallest_and_largest(&range_counts);
    let (key_smallest, key_largest) = smallest_and_largest(&key_counts);

    println!("\n--- the files each map names, over {} keys ---", sample.len());
    println!(
        "the range map:   mean {:.3}, smallest {range_smallest}, largest {range_largest}",
        mean(&range_counts)
    );
    println!(
        "the per-key map: mean {:.3}, smallest {key_smallest}, largest {key_largest}",
        mean(&key_counts)
    );

    let report = serde_json::json!({        "probe": "index-shape",
        "data": arguments.data.display().to_string(),
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "rows": rows,
        "files": shapes.len(),
        "grid_rows": GRID,
        "key_column": KEY,
        "key_descents": descents(&table_keys),
        "distinct_keys": distinct,
        "duplicate_key_rows": table_keys.len().saturating_sub(distinct),
        "samples": sample.len(),
        "range_map_smallest_minimum": smallest_minimum,
        "range_map_largest_maximum": largest_maximum,
        "range_map_files_named_mean": mean(&range_counts),
        "range_map_files_named_smallest": range_smallest,
        "range_map_files_named_largest": range_largest,
        "key_map_files_named_mean": mean(&key_counts),
        "key_map_files_named_smallest": key_smallest,
        "key_map_files_named_largest": key_largest,
        "file_ranges": shapes
            .iter()
            .map(|shape| serde_json::json!({
                "rows": shape.rows,
                "min": shape.min,
                "max": shape.max,
            }))
            .collect::<Vec<_>>(),
    });
    println!("\n--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&arguments.work_dir).ok();
}
