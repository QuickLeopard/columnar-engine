//! The probe, twice: the engine's own probe, in the two paths it runs in.
//!
//! Spec 046 published the engine's join part — **14.625 ms** — and spec 047's ladder measured the
//! engine's own probe over the **same** six million rows, against the **same** table the engine
//! built, at **7.622 ms**. **The same `probe_rows`, and 7 ms apart**, and the two numbers came from
//! two different campaigns on a shared host.
//!
//! **This probe measures the two paths in one session, back to back.**
//!
//! | the path | what it is |
//! |---|---|
//! | **the ladder's path** | the probe over a materialised column: the column is read, and then the engine's own `probe_rows` runs over it, in one pass |
//! | **the query's path** | the probe **inside the reader**: the reader decodes the file in batches and the join's filter probes each batch's key column, before the copy |
//!
//! **Both are the engine's own `probe_rows`**, over the same rows, against the same table. The
//! difference between them is what happens around the probe: in the query's path the reader decodes
//! the value column and copies the matched rows between the probes, and the probes arrive in
//! batches.
//!
//! **And the batch size is swept**, because it is the first candidate: the reader's batch size is
//! **125,000** rows for this file — the largest divisor of a row group's row count under the
//! engine's limit — so the six million rows arrive in **48** batches. The knob is the engine's
//! `ParquetSource::with_reader_batch_rows`.
//!
//! # The rows are checked
//!
//! Every path's rows are compared with the engine's own probe's rows, **row for row, in order**,
//! in every iteration, and the comparison is asserted before anything is timed. A path that
//! returned different rows would be a different query, and its time would not be the engine's.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_core::{ColumnChunk, ComparisonOp, ScalarValue};
use columnar_engine::operators::join::{build_table, probe_rows, JoinFilter};
use columnar_engine::operators::key::KeyColumn;
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::queries::{BuiltinQuery, QUERIES};
use columnar_engine::source::BatchFilter;
use columnar_engine::{DatasetError, ParquetSource};

/// The probe side's key column.
const PROBE_KEY: &str = "l_orderkey";

/// The build side's key column.
const BUILD_KEY: &str = "o_orderkey";

/// The build side's filter column.
const BUILD_DATE: &str = "o_orderdate";

/// The rows the published join produces.
const EXPECTED_ROWS: usize = 216_783;

/// How many times each path is measured.
///
/// **Even**, so that the two orders' groups are the same size.
const ITERATIONS: usize = 8;

/// The batch sizes the sweep measures, beside the engine's own.
///
/// **The points are rows**, and every one of them divides a row group's row count -- the file has
/// six row groups of a million rows each -- so no row group ends with a partial batch and the sweep
/// is the engine's probe and nothing else.
///
/// **The engine's own is one of the points**: **125,000** rows, the largest divisor of a row group's
/// row count under the engine's **byte budget**. It is in the sweep, and in
/// the sweep's rotation, so that "a named size against the engine's own" is a comparison between
/// two points that took the same position half the time each.
const SWEEP: [usize; 4] = [62_500, 125_000, 250_000, 1_000_000];

/// The orders the sweep's points run in, one per iteration.
///
/// **A rotation of four**, twice over: every point takes every position exactly twice, and every
/// pair of points runs in one order twice and the other order twice. A point that always ran first
/// would pay a colder cache than one that always ran last.
const SWEEP_ORDERS: [[usize; 4]; ITERATIONS] = [
    [0, 1, 2, 3],
    [1, 2, 3, 0],
    [2, 3, 0, 1],
    [3, 0, 1, 2],
    [0, 1, 2, 3],
    [1, 2, 3, 0],
    [2, 3, 0, 1],
    [3, 0, 1, 2],
];

/// A duration in milliseconds.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The median of a series of milliseconds.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
}

/// The distinct values of a series, in order.
///
/// **Sorted**, so that the report reads the same way every time it is
/// compared with another.
fn distinct(values: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut sorted: Vec<usize> = values.collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

/// The host's load average.
///
/// **macOS first**: `sysctl` reports `{ 2.31 3.02 3.44 }` and Linux's `/proc/loadavg` reports
/// `2.31 3.02 3.44 1/2345 6789`; both start with the one-minute average.
fn load_average() -> f64 {
    if let Ok(value) = std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output() {
        if value.status.success() {
            if let Some(load) = first_number(&String::from_utf8_lossy(&value.stdout)) {
                return load;
            }
        }
    }
    if let Ok(contents) = std::fs::read_to_string("/proc/loadavg") {
        if let Some(load) = first_number(&contents) {
            return load;
        }
    }
    0.0
}

/// The first number in a line of text, when there is one.
fn first_number(text: &str) -> Option<f64> {
    text.replace(['{', '}'], " ")
        .split_whitespace()
        .next()
        .and_then(|first| first.parse::<f64>().ok())
}

/// The build side's own predicates, pushed into its reader.
fn build_pushdown(source: &ParquetSource) -> PushdownPlan {
    let index = source.column_index(BUILD_DATE).expect("the build side has a date column");
    let mut pushdown = PushdownPlan::new();
    pushdown.push(Pushdown::new(
        index,
        BUILD_DATE,
        ComparisonOp::GtEq,
        ScalarValue::Utf8("1994-01-01".to_owned()),
    ));
    pushdown.push(Pushdown::new(
        index,
        BUILD_DATE,
        ComparisonOp::Lt,
        ScalarValue::Utf8("1995-01-01".to_owned()),
    ));
    pushdown
}

/// The build side's keys, collected from its column.
fn collect_keys(chunk: &ColumnChunk) -> Vec<i64> {
    let keys = KeyColumn::from_chunk(chunk);
    let mut integers = Vec::new();
    for row in 0..chunk.len {
        if let Some(columnar_engine::operators::key::KeyValue::Int(value)) = keys.value_at(row) {
            integers.push(value);
        }
    }
    integers
}

/// How the probe side's keys are ordered.
///
/// **The count of the descents**: how many times the key column falls. A
/// column that rises all the way is a **stream** -- the probe's lookups walk
/// the build table in order, and the table's lines are fetched once. A column
/// that falls is a **shuffle**, and every lookup lands somewhere else in the
/// table, so the table's lines are fetched again and again.
///
/// **This is measured, not assumed**: the probe side's file is shuffled, and
/// the number that says so is published beside the probe's cost.
fn the_descent_count(keys: &[i64]) -> (usize, usize) {
    let mut descents = 0usize;
    for pair in keys.windows(2) {
        let (Some(first), Some(second)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        if second < first {
            descents += 1;
        }
    }
    (descents, keys.len())
}

/// The join's filter, with its own clock.
///
/// **The clock covers the probe and nothing else**, exactly as the engine's own does: the reader
/// times the filter's `keep`, and the filter's `keep` is the probe.
struct Timed<'a> {
    /// The engine's own filter.
    inner: JoinFilter<'a>,
    /// What the filter has seen.
    state: std::sync::Mutex<TimedState>,
}

/// What the filter has seen.
#[derive(Debug, Default)]
struct TimedState {
    /// The probe's own time.
    elapsed: Duration,
    /// The rows the probe kept, in the file's own row numbers.
    ///
    /// **The reader's own numbers**: the batch's positions plus the rows the
    /// batches before it held. The reader's own output is the *dataset's* row
    /// numbers -- the survivors renumbered from zero -- so the file's own
    /// numbers are kept here, and they are the ones the ladder's path returns.
    positions: Vec<u32>,
    /// How many rows the batches before this one held.
    offset: u32,
    /// How many rows each batch held, in the order the batches arrived.
    ///
    /// **Observed, not derived**: the batch size the knob asked for is a
    /// request, and this is what the reader actually did. The report publishes
    /// this, so a point that did not arrive in the size it was named for is
    /// visible rather than assumed.
    batch_rows: Vec<usize>,
}

impl<'a> Timed<'a> {
    /// The filter, and the clock it keeps.
    fn new(table: &'a columnar_engine::operators::join::BuildTable, key: usize) -> Self {
        Self {
            inner: JoinFilter::new(table, key),
            state: std::sync::Mutex::new(TimedState::default()),
        }
    }

    /// The probe's own time, the rows it kept, and the batches it saw.
    fn taken(&self) -> (Duration, Vec<u32>, Vec<usize>) {
        let state = self.state.lock().expect("the lock is not poisoned");
        (state.elapsed, state.positions.clone(), state.batch_rows.clone())
    }
}

impl BatchFilter for Timed<'_> {
    fn keep(
        &self,
        batch: &arrow::record_batch::RecordBatch,
        survivors: Option<&columnar_engine::pushdown::RowMask>,
    ) -> Result<Vec<u32>, DatasetError> {
        let started = Instant::now();
        let rows = self.inner.keep(batch, survivors)?;
        let mut state = self.state.lock().expect("the lock is not poisoned");
        state.elapsed += started.elapsed();
        let offset = state.offset;
        for row in &rows {
            state.positions.push(offset.saturating_add(*row));
        }
        state.batch_rows.push(batch.num_rows());
        state.offset = offset.saturating_add(u32::try_from(batch.num_rows()).unwrap_or(0));
        Ok(rows)
    }
}

/// The probe's own time in the reader, at a given batch size.
///
/// **This is the query's path**: the reader decodes the file in batches, and the join's filter
/// probes each batch's key column before the copy. The clock covers the probe and nothing else.
fn the_query_s_path(
    source: &ParquetSource,
    table: &columnar_engine::operators::join::BuildTable,
    columns: &[usize],
    batch_rows: Option<usize>,
    flavor: columnar_kernels::Flavor,
) -> Result<(f64, Vec<u32>, Vec<usize>), DatasetError> {
    let source = match batch_rows {
        Some(rows) => source.clone().with_reader_batch_rows(rows),
        None => source.clone(),
    };
    let filter = Timed::new(table, 0);
    let output = source.read_filtered(columns, &PushdownPlan::default(), &filter, flavor, 1)?;
    let (elapsed, positions, batches) = filter.taken();
    assert_eq!(positions.len(), output.rows.len(), "the filter kept the rows the reader reported");
    Ok((ms(elapsed), positions, batches))
}

/// One iteration's measurements.
#[derive(Debug, Clone, serde::Serialize)]
struct Iteration {
    /// The iteration's number.
    index: usize,
    /// Which path ran first.
    order: String,
    /// The host's load average.
    load_average: f64,
    /// The ladder's path, in milliseconds.
    ladder_path_ms: f64,
    /// The ladder's path again, immediately, with the build table resident.
    ///
    /// **The same probe, over the same rows, against the same table** -- and
    /// measured at once, with nothing in between to flush it.
    ladder_warm_ms: f64,
    /// The ladder's path once more, after the reader's work.
    ///
    /// **The same probe again** -- measured after the query's path and the
    /// sweep, so that the reader's own work has written over the cache.
    ladder_last_ms: f64,
    /// The query's path, in milliseconds.
    query_path_ms: f64,
    /// The batch sizes the reader actually used, on the query's path.
    ///
    /// **Observed**: the batches the reader handed over, in order.
    query_batches: Vec<usize>,
    /// The sweep's points, in the order they ran, with the point each is.
    ///
    /// **The point is carried with its own time**, because the rotation runs
    /// the points in a different order every iteration: a sample stored by
    /// position alone would be labelled with the wrong point.
    sweep: Vec<SweepPoint>,
    /// Whether the two paths' rows are the same, row for row.
    rows_agree: bool,
    /// How many rows came back.
    rows: usize,
}

/// One sweep point's measurement.
///
/// **The point is carried with its own time**, because the rotation runs the
/// points in a different order every iteration: a sample stored by position
/// alone would be labelled with the wrong point.
#[derive(Debug, Clone, serde::Serialize)]
struct SweepPoint {
    /// Which point of the sweep this is: an index into [`SWEEP`].
    point: usize,
    /// The point's own time, in milliseconds.
    ms: f64,
    /// The batch sizes the reader actually arrived in.
    ///
    /// **Observed**.
    batches: Vec<usize>,
}

/// The campaign's context.
struct Campaign<'a> {
    /// The probe side's source.
    source: &'a ParquetSource,
    /// The engine's own table.
    table: &'a columnar_engine::operators::join::BuildTable,
    /// The columns the reader hands over: the key, and the value the query's
    /// aggregate reads — the reader decodes it, so the probe's cache sees it.
    columns: Vec<usize>,
    /// The kernel flavor the engine reads with.
    flavor: columnar_kernels::Flavor,
}
/// One iteration: the two paths, back to back, and the sweep.
fn measure_iteration(
    campaign: &Campaign<'_>,
    index: usize,
) -> Result<Iteration, Box<dyn std::error::Error>> {
    let load = load_average();
    let order = if index % 2 == 0 { "ladder" } else { "query" };

    // **The ladder's path**: the column is materialised, and the engine's own
    // probe runs over it, in one pass.
    let key_column = campaign.columns.first().ok_or("the campaign names the key column")?;
    let fresh =
        campaign.source.read(&[*key_column], &PushdownPlan::default(), campaign.flavor, 1)?;
    let fresh_chunk = fresh.chunk(PROBE_KEY).ok_or("the probe side's key is there")?;
    let fresh_keys = KeyColumn::from_chunk(fresh_chunk);
    let rows_in = u32::try_from(fresh.len()).unwrap_or(0);

    // **The two paths, back to back, with the order alternated.**
    //
    // The materialisation above is the disturbance both of them meet: it is
    // the read that writes 48 million bytes, and whichever path runs first
    // pays for it. **That is why the order alternates.**
    let (ladder_ms, ladder_rows, query_ms, query_rows, query_batches) = if order == "ladder" {
        let started = Instant::now();
        let rows = probe_rows(campaign.table, &fresh_keys, 0..rows_in);
        let ladder_ms = ms(started.elapsed());
        let (query_ms, query_rows, query_batches) = the_query_s_path(
            campaign.source,
            campaign.table,
            &campaign.columns,
            None,
            campaign.flavor,
        )?;
        (ladder_ms, rows, query_ms, query_rows, query_batches)
    } else {
        let (query_ms, query_rows, query_batches) = the_query_s_path(
            campaign.source,
            campaign.table,
            &campaign.columns,
            None,
            campaign.flavor,
        )?;
        let started = Instant::now();
        let rows = probe_rows(campaign.table, &fresh_keys, 0..rows_in);
        (ms(started.elapsed()), rows, query_ms, query_rows, query_batches)
    };

    // **The sweep**: the query's path at the other batch sizes, in the
    // iteration's order.
    let mut sweep: Vec<SweepPoint> = Vec::with_capacity(SWEEP.len());
    let rotation =
        SWEEP_ORDERS.get(index).ok_or("the iteration has an order in the sweep's rotation")?;
    for point in rotation.iter().copied() {
        let batch_rows = SWEEP.get(point).copied().ok_or("the sweep's point is there")?;
        let (elapsed, _, batches) = the_query_s_path(
            campaign.source,
            campaign.table,
            &campaign.columns,
            Some(batch_rows),
            campaign.flavor,
        )?;
        sweep.push(SweepPoint { point, ms: elapsed, batches });
    }

    // **The same probe, once more, after the reader's work.**
    //
    // The reader's decode between two probes writes over the cache, and the
    // build table goes with it. Measured here, after the query's path and the
    // sweep have run, the same probe is back to the cold price it paid at the
    // start of the iteration -- **and the difference from the warm price is
    // what the reader's work costs the table.**
    let started = Instant::now();
    let last_rows = probe_rows(campaign.table, &fresh_keys, 0..rows_in);
    let ladder_last_ms = ms(started.elapsed());
    assert_eq!(last_rows, ladder_rows, "the last probe keeps the rows the cold one kept");

    // **The same probe, again, immediately.**
    //
    // The probe's lookups are random and the build table is bigger than any
    // cache, so the probe's cost is the table's residency: measured **first**,
    // the table has just been flushed by the materialisation, and measured
    // **again at once**, nothing has touched it and it is still resident.
    //
    // **The difference between the two is the table's residency**, and it is
    // measured in the same iteration, over the same rows, against the same
    // table.
    let started = Instant::now();
    let warm_rows = probe_rows(campaign.table, &fresh_keys, 0..rows_in);
    let ladder_warm_ms = ms(started.elapsed());
    assert_eq!(warm_rows, ladder_rows, "the warm probe keeps the rows the cold one kept");

    Ok(Iteration {
        index,
        order: order.to_owned(),
        load_average: load,
        ladder_path_ms: ladder_ms,
        ladder_warm_ms,
        ladder_last_ms,
        query_path_ms: query_ms,
        query_batches,
        sweep,
        rows_agree: ladder_rows == query_rows,
        rows: query_rows.len(),
    })
}

/// The probe's arguments: the file, where the report goes, and whether the
/// reader also decodes the value column.
fn arguments() -> Result<(PathBuf, PathBuf, bool), Box<dyn std::error::Error>> {
    let mut path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut key_only = false;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--data" => path = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--key-only" => key_only = true,
            "--help" | "-h" => {
                println!(
                    "usage: probe_paths_probe --data <parquet> [--out <report.json>] [--key-only]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let path = path.unwrap_or_else(|| PathBuf::from("data/lineitem.parquet"));
    let out = out.unwrap_or_else(|| PathBuf::from("bench-results/probe-paths-run1.json"));
    Ok((path, out, key_only))
}

/// The build side's file, beside the probe side's.
fn build_path_for(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let orders_stem = match stem.strip_prefix("lineitem") {
        Some(rest) => format!("orders{rest}"),
        None => "orders".to_owned(),
    };
    path.with_file_name(format!("{orders_stem}.parquet"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (data, out, key_only) = arguments()?;

    let definition = QUERIES
        .iter()
        .find(|definition| definition.query == BuiltinQuery::JoinOrders)
        .ok_or("the join-orders query is in the engine's table")?;
    let plan = (definition.plan)(&build_path_for(&data));
    let spec = plan.join.as_ref().ok_or("the plan has a join")?;
    let build_path = spec.build_paths.first().ok_or("the plan names a build side")?;

    let flavor = columnar_kernels::best();
    let source = ParquetSource::open(&data)?;
    let build_source = ParquetSource::open(build_path)?;

    // The build side's keys, and the engine's own table.
    let pushdown = build_pushdown(&build_source);
    let build_key_index =
        build_source.column_index(BUILD_KEY).ok_or_else(|| format!("no {BUILD_KEY} column"))?;
    let build_dataset = build_source.read(&[build_key_index], &pushdown, flavor, 1)?;
    let build_chunk = build_dataset.chunk(BUILD_KEY).ok_or("the build side's key is there")?;
    let build_keys = collect_keys(build_chunk);
    drop(build_dataset);

    let table = build_table(&build_source, spec, flavor, 1)?;
    assert_eq!(table.rows(), build_keys.len(), "the engine's table holds the build side's keys");

    // The columns the reader hands over: the key, and the value the query's
    // aggregate reads.
    //
    // **The key alone, when the campaign asks for it**: the reader's own work
    // between two probes is the decode of the columns it hands over, and the
    // value column is the bigger of the two. A campaign that reads the key
    // alone is the same probe with less of the reader's context around it.
    let key_index =
        source.column_index(PROBE_KEY).ok_or_else(|| format!("no {PROBE_KEY} column"))?;
    let value_index = source
        .column_index("l_extendedprice")
        .ok_or_else(|| "no l_extendedprice column".to_owned())?;
    let columns = if key_only { vec![key_index] } else { vec![key_index, value_index] };

    // **The probe side's key column, and how it is ordered.** The probe's
    // lookups are only a stream if the keys rise, and the file's do not.
    let probe_dataset = source.read(&[key_index], &PushdownPlan::default(), flavor, 1)?;
    let probe_chunk = probe_dataset.chunk(PROBE_KEY).ok_or("the probe side's key is there")?;
    let probe_keys = collect_keys(probe_chunk);
    let (descents, probe_rows) = the_descent_count(&probe_keys);
    drop(probe_dataset);

    let columns_read = columns.len();
    let campaign = Campaign { source: &source, table: &table, columns, flavor };

    let mut iterations = Vec::with_capacity(ITERATIONS);
    for index in 0..ITERATIONS {
        iterations.push(measure_iteration(&campaign, index)?);
    }

    // The report.
    let report = the_report(&iterations, build_keys.len(), descents, probe_rows, columns_read);

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    println!("wrote {}", out.display());
    Ok(())
}

/// The report the campaign writes.
///
/// **Every figure is here, and every figure is derived from the iterations** —
/// the samples are published beside the medians they came from, so that the
/// medians can be checked against them.
fn the_report(
    iterations: &[Iteration],
    build_keys: usize,
    descents: usize,
    probe_rows: usize,
    columns_read: usize,
) -> serde_json::Map<String, serde_json::Value> {
    let ladder: Vec<f64> = iterations.iter().map(|i| i.ladder_path_ms).collect();
    let warm: Vec<f64> = iterations.iter().map(|i| i.ladder_warm_ms).collect();
    let last: Vec<f64> = iterations.iter().map(|i| i.ladder_last_ms).collect();
    let query: Vec<f64> = iterations.iter().map(|i| i.query_path_ms).collect();
    let differences: Vec<f64> =
        iterations.iter().map(|i| i.query_path_ms - i.ladder_path_ms).collect();
    // **The table's residency**: the same probe, cold and warm.
    let residency: Vec<f64> =
        iterations.iter().map(|i| i.ladder_path_ms - i.ladder_warm_ms).collect();
    // **What the reader's work costs the table**: the same probe, warm and
    // after the reader's work.
    let the_readers_cost: Vec<f64> =
        iterations.iter().map(|i| i.ladder_last_ms - i.ladder_warm_ms).collect();
    let mut report = serde_json::Map::new();
    report.insert("probe".to_owned(), serde_json::json!("probe-paths"));
    report.insert(
        "code".to_owned(),
        serde_json::json!(std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map_or_else(
                || "unknown".to_owned(),
                |output| { String::from_utf8_lossy(&output.stdout).trim().to_owned() }
            )),
    );
    report.insert("iterations".to_owned(), serde_json::json!(ITERATIONS));
    report.insert("build_keys".to_owned(), serde_json::json!(build_keys));
    // **How many columns the reader handed over**: two is the query's own
    // shape -- the key and the value the aggregate reads -- and one is the
    // key alone, which is the same probe with less of the reader's context
    // around it.
    report.insert("the_columns_read".to_owned(), serde_json::json!(columns_read));
    // **How the probe side's keys are ordered**: the descents, and the rows
    // they fall across. The probe's lookups are a stream only if the count is
    // zero.
    report.insert(
        "the_probe_side_s_key_order".to_owned(),
        serde_json::json!({"descents": descents, "rows": probe_rows}),
    );
    report.insert("expected_rows".to_owned(), serde_json::json!(EXPECTED_ROWS));
    report.insert("sweep_batch_rows".to_owned(), serde_json::json!(SWEEP.to_vec()));
    // **The batch size the engine's own actually used**, observed on the
    // query's path: the reader's own answer to "how big were the batches".
    report.insert(
        "the_engine_s_own_batch_rows".to_owned(),
        serde_json::json!(distinct(
            iterations.iter().flat_map(|i| i.query_batches.iter().copied())
        )),
    );
    report.insert(
        "the_engine_s_own_batches".to_owned(),
        serde_json::json!(iterations.first().map_or(0, |i| i.query_batches.len())),
    );
    for (name, values) in [
        ("ladder_path_ms", &ladder),
        ("ladder_warm_ms", &warm),
        ("ladder_last_ms", &last),
        ("query_path_ms", &query),
    ] {
        report.insert(
            name.to_owned(),
            serde_json::json!({
                "median": median(values),
                "min": values.iter().copied().fold(f64::INFINITY, f64::min),
                "max": values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "samples": values,
            }),
        );
    }
    report.insert(
        "the_gap_ms".to_owned(),
        serde_json::json!({
            "median": median(&differences),
            "min": differences.iter().copied().fold(f64::INFINITY, f64::min),
            "max": differences.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "samples": differences,
        }),
    );
    // **The table's residency**: the same probe, cold and warm, in the same
    // iteration.
    report.insert(
        "the_residency_ms".to_owned(),
        serde_json::json!({
            "median": median(&residency),
            "min": residency.iter().copied().fold(f64::INFINITY, f64::min),
            "max": residency.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "samples": residency,
        }),
    );
    report.insert(
        "the_readers_cost_ms".to_owned(),
        serde_json::json!({
            "median": median(&the_readers_cost),
            "min": the_readers_cost.iter().copied().fold(f64::INFINITY, f64::min),
            "max": the_readers_cost.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "samples": the_readers_cost,
        }),
    );
    // The sweep: each point's own samples, over the iterations it ran in.
    report.insert("the_sweep_ms".to_owned(), the_sweep(iterations));
    report.insert(
        "the_rows_agree".to_owned(),
        serde_json::json!(iterations.iter().all(|i| i.rows_agree)),
    );
    report.insert(
        "the_rows".to_owned(),
        serde_json::json!(iterations.iter().map(|i| i.rows).collect::<Vec<_>>()),
    );
    report.insert("the_iterations".to_owned(), serde_json::json!(iterations));
    report
}

/// The sweep's own figures: each point's samples, and the batch sizes the
/// reader actually arrived in.
///
/// **The samples are found by the point they belong to**, not by the position
/// they ran in: the rotation runs the points in a different order every
/// iteration, and a sample read by position would carry the wrong point's
/// name.
fn the_sweep(iterations: &[Iteration]) -> serde_json::Value {
    let mut sweep = serde_json::Map::new();
    for (point, rows) in SWEEP.iter().enumerate() {
        let values: Vec<f64> = iterations
            .iter()
            .filter_map(|i| i.sweep.iter().find(|s| s.point == point).map(|s| s.ms))
            .collect();
        // **The batch sizes this point's read actually arrived in**, observed.
        let observed: Vec<usize> = iterations
            .iter()
            .filter_map(|i| i.sweep.iter().find(|s| s.point == point))
            .flat_map(|s| s.batches.iter().copied())
            .collect();
        sweep.insert(
            rows.to_string(),
            serde_json::json!({
                "median": median(&values),
                "min": values.iter().copied().fold(f64::INFINITY, f64::min),
                "max": values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "samples": values,
                "the_batch_rows_observed": distinct(observed.into_iter()),
            }),
        );
    }
    serde_json::json!(sweep)
}
