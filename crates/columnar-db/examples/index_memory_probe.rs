//! The index's memory.
//!
//! ```text
//! cargo run --release -p columnar-db --example index_memory_probe -- \
//!     --data data/lineitem.parquet --out bench-results/index-memory-run1.json
//! ```
//!
//! # The question
//!
//! **The index's price has a memory axis, and the axis is the table's keys.**
//! The index is a map from the key to the files that hold it, so **it holds an
//! entry a key** — **921,129** of them over the million rows the database
//! writes, published in
//! [§3am](../../../PERF.md#3am-the-indexs-shape-measured-before-it-is-built).
//!
//! **And the memory it holds is not the engine's read's memory.** The read
//! that a lookup makes is bounded by the engine's budget — **256 MiB** — and
//! the index is not that read: it is a map that **lives as long as the
//! snapshot**, and **the snapshot's lifetime is not the read's lifetime.**
//!
//! **So the question is the one the compaction's ceiling asked, in the same
//! shape**: **at what row count does the index's own memory pass the
//! engine's budget?** And **the answer is measured, and not derived from a
//! guess about the map's layout.**
//!
//! # The measurement
//!
//! **A counting global allocator.** It sees every allocation the process
//! makes, and it sees the **peak** — which is what a memory budget has to
//! survive. **The bytes the index holds are the difference between what the
//! process holds before the lookup and what it holds after it**, because the
//! index is kept by the snapshot and the snapshot outlives the measurement.
//!
//! **And the allocator is in this example, not in the database.** Every crate
//! here denies `unsafe_code`, and a `GlobalAlloc` implementation needs
//! `unsafe`. **An example is its own crate**, so the counting allocator is
//! the diagnostic's and the database's own memory behaviour is untouched by
//! it.
//!
//! **And the time is not measured here.** A counting allocator counts **every**
//! allocation, and that is not free — so the memory is measured with the
//! allocator in it, and **the time is measured in `index_probe.rs`, which has
//! no allocator in it.** A time measured next to an allocator is not the
//! engine's time.
//!
//! # The table
//!
//! **The key column alone.** The index is over the key column, so **its
//! memory is a function of the keys** — and the table's other columns are the
//! scan's business and not the index's. **So the table holds the key column
//! and the grid**, and the row counts are the row counts of the table the
//! database writes.

#![allow(clippy::indexing_slicing, clippy::cast_precision_loss)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[path = "support/counting_allocator.rs"]
mod counting_allocator;

use columnar_db::prelude::*;
use columnar_engine::{ExecOptions, FileSetSource, MemoryBudget, QueryPlan, ScalarValue};
use counting_allocator::Counting;

/// The allocator that counts.
///
/// **It is here, in the example, and not in the database**: every crate here
/// denies `unsafe_code`, and a `GlobalAlloc` implementation needs `unsafe`.
#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The key column: the point lookup's column.
const KEY: &str = "l_orderkey";

/// The table the probe writes.
const TABLE: &str = "many";

/// The grid: the store's own, and the rows' own batch.
const GRID: usize = columnar_store::BATCH_ROWS;

/// The rows the probe's table holds.
const DEFAULT_ROWS: usize = 1_000_000;

/// How many steps the bisection for the ceiling takes.
const CEILING_STEPS: usize = 12;

/// The probe's arguments.
struct Arguments {
    /// The file the probe reads its keys from.
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
    let mut work_dir = PathBuf::from("target/index-memory-probe");
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

/// The table's schema: the key column, and nothing else.
fn schema() -> Schema {
    Schema::new(vec![Column::key(KEY, DataType::Int64)])
        .expect("the schema is one a table can hold")
}

/// The keys of the source file, in the file's order.
fn keys_of(path: &Path) -> Vec<i64> {
    let source = FileSetSource::open(&[path.to_path_buf()]).expect("the source opens");
    let plan = QueryPlan::new(Vec::new(), vec![KEY.to_owned()], None);
    let dataset = source.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");
    let chunk = dataset.chunk(KEY).expect("the file holds the column");
    match &chunk.encoding {
        columnar_engine::Encoding::Int64 { values } => values.clone(),
        columnar_engine::Encoding::Int64Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                *dict.get(position).expect("the index is in range")
            })
            .collect(),
        other => panic!("the column {KEY} is not an integer: {other:?}"),
    }
}

/// What the index held, over a table of so many rows.
struct Measurement {
    /// The rows the table held.
    rows: usize,
    /// The bytes the index holds, live.
    bytes: usize,
    /// The bytes the build held at its peak, above the baseline.
    peak: usize,
    /// The entries the index holds.
    entries: usize,
}

/// Writes the table, takes a snapshot, and measures what the index holds.
///
/// **And the bytes are the difference between what the process holds before
/// the lookup and what it holds after it**: the index is kept by the
/// snapshot, so what it holds is still held when the measurement is taken.
fn measure(keys: &[i64], rows: usize, work_dir: &Path) -> Measurement {
    let root = work_dir.join("memory");
    std::fs::remove_dir_all(&root).ok();
    let mut database = Database::open(&root).expect("the database opens");
    {
        let mut table = database.create_table(TABLE, schema()).expect("the table is created");
        let mut written = 0;
        while written < rows {
            let count = GRID.min(rows - written);
            let batch: Vec<Row> = keys[written..written + count]
                .iter()
                .map(|key| Row::filled(vec![ScalarValue::Int64(*key)]))
                .collect();
            table.insert(&batch).expect("the rows are accepted");
            written += count;
        }
        table.commit().expect("the rows are written");
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table(TABLE).expect("the table is there");
    let key = keys[rows / 2];

    // **The baseline**: what the process holds, with the table written and the
    // snapshot taken.
    let baseline = Counting::held();
    Counting::reset(baseline);

    // **The lookup**: the index's build, and the read.
    let rows_out = table.lookup(key).expect("the lookup answers");
    let _ = rows_out.len();

    // **And what it holds now**, and what it held at its peak.
    let held = Counting::held();
    let peak = Counting::peak();
    std::fs::remove_dir_all(&root).ok();

    // **And the entries**, counted after the measurement — so the set that
    // counts them is not in the bytes the index holds.
    let entries = keys[..rows].iter().copied().collect::<HashSet<_>>().len();

    Measurement {
        rows,
        bytes: held.saturating_sub(baseline),
        peak: peak.saturating_sub(baseline),
        entries,
    }
}

/// The row count at which the index's memory passes the budget.
struct Ceiling {
    /// The largest row count whose index holds **less** than the budget.
    under: usize,
    /// The smallest row count whose index holds **more** than the budget --
    /// **and zero when nothing passed it**, which is the answer the index
    /// gives on a table this size.
    over: usize,
    /// How many steps the bisection took.
    steps: usize,
}

/// The ceiling, found by bisection.
///
/// **And both bounds are measured**: the row count the probe's own table
/// holds is measured first, and the whole source is measured too -- **and the
/// bisection runs only between two measured bounds.**
fn ceiling(keys: &[i64], work_dir: &Path, lower: usize, budget: usize) -> Ceiling {
    let top = keys.len();
    let mut found = Ceiling { under: lower, over: 0, steps: 0 };
    if top <= lower {
        return found;
    }

    // **The top**: the whole source.
    found.steps += 1;
    if measure(keys, top, work_dir).bytes <= budget {
        // **And nothing passed it**: the index never passes the budget on a
        // table this size.
        found.under = top;
        return found;
    }
    found.over = top;

    // **And the lower bound**: measured, not assumed.
    if measure(keys, lower, work_dir).bytes > budget {
        found.under = 0;
        found.over = lower;
        return found;
    }

    while found.steps < CEILING_STEPS {
        let candidate = (found.under + found.over) / 2;
        if candidate <= found.under {
            break;
        }
        found.steps += 1;
        if measure(keys, candidate, work_dir).bytes > budget {
            found.over = candidate;
        } else {
            found.under = candidate;
        }
    }
    found
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

/// The measurement, as the report carries it.
fn figure_of(measurement: &Measurement) -> serde_json::Value {
    serde_json::json!({
        "rows": measurement.rows,
        "bytes": measurement.bytes,
        "peak_bytes": measurement.peak,
        "entries": measurement.entries,
        "bytes_per_entry": if measurement.entries == 0 {
            0.0
        } else {
            measurement.bytes as f64 / measurement.entries as f64
        },
    })
}

fn main() {
    let arguments = arguments();
    println!("data: {}", arguments.data.display());
    println!("grid: {GRID} rows");

    // **The engine's own budget**, and not a copy of the number.
    let budget = MemoryBudget::default().bytes;
    println!("the engine's budget: {budget} bytes");

    let keys = keys_of(&arguments.data);
    println!("--- the source's own keys, read once ---");
    println!("keys: {}\n", keys.len());

    let rows = arguments.rows.min(keys.len());
    let measured = measure(&keys, rows, &arguments.work_dir);
    println!("--- the index's memory, over {rows} rows ---");
    println!("the bytes it holds:  {:>14}", measured.bytes);
    println!("the peak it held:   {:>14}", measured.peak);
    println!("the entries:        {:>14}", measured.entries);
    println!(
        "the bytes an entry: {:>14.1}",
        measured.bytes as f64 / measured.entries.max(1) as f64
    );

    println!("--- the ceiling: the row count at which the index passes the budget ---");
    let found = ceiling(&keys, &arguments.work_dir, rows, budget);
    println!("under: {} rows, over: {} rows, steps: {}", found.under, found.over, found.steps);

    let report = serde_json::json!({
        "probe": "index-memory",
        "data": arguments.data.display().to_string(),
        "commit": commit_sha(),
        "host_load_average": load_average(),
        "budget_bytes": budget,
        "rows": rows,
        "index_bytes": measured.bytes,
        "index_peak_bytes": measured.peak,
        "index_entries": measured.entries,
        "index_bytes_per_entry": measured.bytes as f64 / measured.entries.max(1) as f64,
        "ceiling_under_rows": found.under,
        "ceiling_over_rows": found.over,
        "ceiling_steps": found.steps,
        "measurement": figure_of(&measured),
    });
    println!("\n--- the report ---");
    println!("{report}");
    if let Some(out) = arguments.out {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }

    std::fs::remove_dir_all(&arguments.work_dir).ok();
}
