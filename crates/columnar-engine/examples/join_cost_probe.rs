//! The join's stage, taken apart.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind spec 026's question: **what is in the join's stage, and what is the
//! difference between the two paths that report it?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example join_cost_probe -- \
//!     data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! The plan output is the engine's own account of its work, and it says the
//! join's stage is **28.5 ms** of the resident `join-orders` and **29.9 ms**
//! of the cold one -- and the two paths differ by **2.9 ms**. The stage is
//! measured against its own parts here, in one session, on the same data, on
//! the same rows.
//!
//! # The parts
//!
//! | | |
//! |---|---|
//! | **`stage_cold`**, **`stage_resident`** | the engine's own join stage, as the plan output reports it, on both paths |
//! | **`build_open`** | opening `orders.parquet`: its metadata, its schema |
//! | **`build_read`** | reading the build side's column through the engine's own reader |
//! | **`build_keys`** | the per-row walk over the build side that collects its keys |
//! | **`build_table`** | the whole of the build side: the read, the keys and the table together |
//! | **`table_own`** | the table's own construction: one byte a count, over the key's own range |
//! | **`probe_column`** | the probe over the engine's own column, in memory: the **resident** path's probe |
//! | **`probe_batches`** | the probe inside the reader's own batch loop: the **cold** path's probe |
//!
//! | **`candidate_probe`** | the same lookup, written out by hand, over the same rows and the same table |
//!
//! The **table**, the **output** and the probe's **remainder** are the
//! differences: the table is what `build_whole` costs over the read and the
//! keys, the output is what the candidate probe costs over the floor, and the
//! remainder is what the engine's own probe costs over the candidate's.
//!
//! # The remainder, taken apart
//!
//! | | |
//! |---|---|
//! | **`candidate_with_map`** | the candidate **with the overflow map's consult** in it: the same lookup, over the same rows and the same table, and one thing more -- the map the engine's own table consults once a probe row |
//! | **`cold_candidate`** | the candidate **inside the reader's own batch loop**: the same hand-written lookup, over the reader's own Arrow batches, so that the cold path's remainder is measurable at all |
//!
//! The **map's share** is what the candidate with the consult costs over the
//! candidate without it, and the **cold remainder** is what the engine's probe
//! in the batches costs over the cold candidate's. Nothing else is different
//! between the two candidates, so the difference between them is what the one
//! thing that is different costs -- **measured, not assumed**.
//!
//! # The ladder
//!
//! The remainder is **3.337 ms** and it is not the map. What is left is the
//! lookup's **shape**, and a list of names is not a decomposition: the parts
//! have to be **measured**, one at a time, over the same rows and the same
//! table, with each candidate differing from the one before it in exactly one
//! thing.
//!
//! | the rung | what it is | the increment it prices |
//! |---|---|---|
//! | **`candidate_probe`** | the plain candidate: the byte, as a `u8`, through `unwrap_or` | -- |
//! | **`candidate_chain`** | the same read, folded through `map_or` | the **`Option` chain** |
//! | **`candidate_width`** | the byte widened to `u32`, the way the engine's is | the **`u32` width** |
//! | **`candidate_state`** | the same read, but the table is a struct and the count comes from a method over `&self` | **the state through `self`** |
//! | **`candidate_map_option`** | the same, with the overflow map's `Option` read and branched on | **the map's discriminant** |
//! | **`engine_lookup_plain`** | the engine's **own** lookup, over the instrument's own key source | **the engine's loop** |
//!
//! And the last step: the engine's probe over the engine's own key column,
//! which is **`probe_column`** -- the difference between it and
//! **`engine_lookup_plain`** is the **key source**.
//!
//! The six increments add up to the remainder by construction, so what the
//! ladder says is **where** the remainder is, and the difference between the
//! sum of the increments and the remainder is the noise the decomposition
//! carries. Every rung's rows are checked against the engine's rows, in order,
//! before anything is timed.
//!
//! # Every rung reads a column materialised afresh
//!
//! The engine's own query materialises the probe column and then probes it, so
//! the column it probes is one the reader has **just written**. A rung that
//! read a buffer kept warm across the iterations would be measured on a
//! **cheaper** column than the engine pays for, and the difference between the
//! two would be the **buffer's**, not the lookup's. Every rung that touches
//! the column therefore materialises it afresh, and times only the probe.
//!
//! # The stage's own residual
//!
//! The stage's **residual** is what the stage leaves after the build side's
//! whole and its open -- the stage's own probe, and the output it gathers --
//! and the **context** is the gap between that residual and the standalone
//! probe of the same work: the price of running inside a query rather than in
//! a quiet process. Both are published, because the residual is not the probe
//! and the difference between them changes sign between sets.
//!
//! # The controls
//!
//! | | |
//! |---|---|
//! | **`read_plain`** | the plain read of the probe key column: the floor a read cannot go under |
//! | **`read_small`** | the same read over a **small** buffer: the control that says the read is not memory-bound |
//! | **`floor_probe`** | the **floor the probe cannot go below**: the table's own load, one a probe row |
//!
//! # The two paths are compared
//!
//! The cold path's probe runs **inside** the reader, over the reader's own
//! batches, against the caches the decode is using. The resident path's probe
//! runs over the engine's own column, in memory. The probe measures **both**,
//! over the same rows and against the same table, and the difference between
//! them is the cold path's extra cost.
//!
//! # The answer is checked first
//!
//! The probe runs the engine's **own** plan through the engine's own entry
//! point and checks the answer against the published one before anything is
//! timed. The probe's own table and its own lookup are checked against the
//! engine's rows, **row for row and in order**, before they are timed: a
//! candidate that answers differently is not a candidate, it is a different
//! join.
//!
//! # What the figures are not
//!
//! These are figures on a **shared** host, taken five times each and reported
//! as the median with the spread beside it. The report carries the host's load
//! average, so a figure that drifted can be read against it.

// The report carries a figure for every part of the ladder, and `serde_json`'s
// `json!` macro counts its own expansion against the recursion limit.
#![recursion_limit = "512"]

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::array::Int64Array;
use columnar_core::{ColumnChunk, ComparisonOp, ScalarValue};
use columnar_engine::operators::join::{build_table, probe_rows, BuildTable};
use columnar_engine::operators::key::{ArrowKeys, KeyColumn, KeySource, KeyValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::queries::BuiltinQuery;
use columnar_engine::{Dataset, ExecOptions, JoinSpec, ParquetSource, MAX_READER_BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;

/// The report's own map of figures.
type Figures = serde_json::Map<String, serde_json::Value>;

/// The probe side's key column.
const PROBE_KEY: &str = "l_orderkey";

/// The build side's key column.
const BUILD_KEY: &str = "o_orderkey";

/// The build side's filter column.
const BUILD_DATE: &str = "o_orderdate";

/// The answer the published join gives.
///
/// The sum of `l_extendedprice` over the rows the join produces.
const EXPECTED: f64 = 11_472_226_962.890_1;

/// The rows the published join produces.
const EXPECTED_ROWS: usize = 216_783;

/// How many times each part is measured.
const ITERATIONS: usize = 5;

/// How many keys the small control holds.
///
/// It is small enough to be in the caches whatever happens, so a read that
/// costs the same over it as over the whole column is not waiting for memory.
const SMALL_KEYS: usize = 512 * 1024;

/// A timing, and the spread it was taken over.
#[derive(Clone, Copy)]
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
        Self {
            median: ms(sorted.get(middle).copied().unwrap_or_default()),
            min: ms(sorted.first().copied().unwrap_or_default()),
            max: ms(sorted.last().copied().unwrap_or_default()),
        }
    }

    /// The timing's spread, as the report carries it.
    fn spread(self) -> [f64; 2] {
        [self.min, self.max]
    }
}

/// A duration, in milliseconds.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// The median of a series of milliseconds.
///
/// The ladder's series are differences, not durations, so they are already in
/// milliseconds and they are reduced the same way the durations are.
fn median_ms(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    sorted.get(middle).copied().unwrap_or_default()
}

/// Repeats a measurement, and returns its median and its spread.
fn repeat(iterations: usize, mut measure: impl FnMut() -> Duration) -> Timing {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        samples.push(measure());
    }
    Timing::of(&samples)
}

/// How many clock reads one sample of the clock's own cost takes.
const CLOCK_READS: u32 = 1_000;

/// The cost of one clock read.
///
/// The engine's resident path reads the clock twice more than it used to, and
/// the route that reports the stage's parts is only taken if what those reads
/// cost is inside the threshold. So the cost is **measured**: a thousand reads
/// are taken between two clocks, and the outer clock's own cost is amortised
/// over all of them and is not what is being measured.
fn clock_cost() -> Timing {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let mut last = Instant::now();
        for _ in 0..CLOCK_READS {
            last = Instant::now();
        }
        let elapsed = started.elapsed();
        std::hint::black_box(last);
        samples.push(elapsed / CLOCK_READS);
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

/// The tree's state, for the report.
///
/// The commit alone does not distinguish two codes: spec 029's before-set and
/// its after-set carry **the same commit**, because the before-set's tree was
/// dirty. So the tree's state is carried beside the commit, and the campaign
/// names the code it measured.
fn tree_state() -> String {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map_or_else(
        || "unknown".to_owned(),
        |output| {
            if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
                "clean".to_owned()
            } else {
                "dirty".to_owned()
            }
        },
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
        .map(|text| text.replace(['{', '}'], " "))
        .and_then(|text| text.split_whitespace().next().and_then(|first| first.parse::<f64>().ok()))
        .unwrap_or(0.0)
}

/// The definition of the published join.
fn definition() -> &'static columnar_engine::queries::QueryDefinition {
    columnar_engine::queries::QUERIES
        .iter()
        .find(|definition| definition.query == BuiltinQuery::JoinOrders)
        .expect("the join is a builtin one")
}

/// The aggregate's value, as the number the published campaigns recorded.
fn value_of(result: &columnar_engine::AggregateResult) -> f64 {
    result.groups.first().and_then(|group| group.values.first()).map_or(f64::NAN, |value| {
        match value {
            columnar_engine::AggregateValue::Float(v) => *v,
            columnar_engine::AggregateValue::Int(v) => {
                f64::from(u32::try_from(*v).unwrap_or(u32::MAX))
            }
            columnar_engine::AggregateValue::Null => f64::NAN,
        }
    })
}

/// The join's stage, as the plan output reports it.
///
/// The stage is the engine's own account, so it is read from the plan output
/// rather than measured beside it.
fn stage_of(result: &columnar_engine::AggregateResult) -> Duration {
    result
        .stages
        .iter()
        .find(|stage| stage.stage == "join")
        .map_or(Duration::ZERO, |stage| stage.elapsed)
}

/// The rows the join's stage reports, as the plan output has them.
///
/// The two numbers are the rows the stage was **handed** and the rows it
/// **produced**. Before this spec's fix the two paths reported different
/// numbers for the same work, and the report carries both so that a
/// difference can be seen rather than argued about.
fn stage_rows_of(result: &columnar_engine::AggregateResult) -> (u64, u64) {
    result
        .stages
        .iter()
        .find(|stage| stage.stage == "join")
        .map_or((0, 0), |stage| (stage.rows_in, stage.rows_out))
}

/// Checks a query's answer against the published one.
fn check(result: &columnar_engine::AggregateResult) {
    let value = value_of(result);
    let tolerance = EXPECTED.abs().max(1.0) * 1e-6;
    assert!(
        (value - EXPECTED).abs() <= tolerance,
        "the join's answer is {value}, the published answer is {EXPECTED}"
    );
    let (_, rows) = stage_rows_of(result);
    assert_eq!(
        usize::try_from(rows).unwrap_or(usize::MAX),
        EXPECTED_ROWS,
        "the join produced {rows} rows, the published join produced {EXPECTED_ROWS}"
    );
}

/// The join stage's own parts, as the engine's plan output reports them.
#[derive(Clone, Copy)]
struct StageParts {
    /// The build table's construction.
    build: Duration,
    /// The probe.
    probe: Duration,
    /// The build side's file open.
    open: Duration,
}

/// The join stage's parts, from the plan output.
///
/// The parts are the engine's own account of the stage, and they are **absent**
/// when the engine does not report them — which is the case for the code this
/// spec replaces, and the reason the ladder is computed from what the engine
/// reports and never from what it does not.
fn parts_of(result: &columnar_engine::AggregateResult) -> Option<StageParts> {
    let join = result.plan_output.join.as_ref()?;
    Some(StageParts {
        build: join.build_elapsed,
        probe: join.probe_elapsed,
        open: join.open_elapsed,
    })
}

/// One path's own measurements: the stage and its parts, **per iteration**.
///
/// The parts are kept per iteration because the ladder is built from them per
/// iteration: the stage's elapsed is the three parts **added together** in
/// every one of them, and a median of a sum is not the sum of the medians.
/// The per-iteration series is what makes the ladder close.
struct PathMeasurements {
    /// The stage's elapsed, one per iteration.
    stage: Vec<Duration>,
    /// The build table's construction, one per iteration.
    build: Vec<Duration>,
    /// The probe, one per iteration.
    probe: Vec<Duration>,
    /// The build side's file open, one per iteration.
    open: Vec<Duration>,
}

impl PathMeasurements {
    /// The stage's elapsed, as a timing.
    fn stage_timing(&self) -> Timing {
        Timing::of(&self.stage)
    }

    /// The stage's parts, as a timing, when the engine reports them.
    fn parts(&self) -> Option<(Timing, Timing, Timing)> {
        (!self.build.is_empty())
            .then(|| (Timing::of(&self.build), Timing::of(&self.probe), Timing::of(&self.open)))
    }
}

/// What the engine's own plan output says the join stage did.
struct EngineStages {
    /// The cold path's own measurements.
    cold: PathMeasurements,
    /// The resident path's own measurements.
    resident: PathMeasurements,
    /// The rows the join's stage reports, cold.
    cold_rows: (u64, u64),
    /// The rows the join's stage reports, resident.
    resident_rows: (u64, u64),
}

/// The engine's own join stage, on both paths.
///
/// The cold path is the one where the scan feeds the join, and the resident
/// path is the one where the join probes the dataset itself.
fn engine_stages(
    plan: &columnar_engine::QueryPlan,
    source: &ParquetSource,
    dataset: &Dataset,
    options: &ExecOptions,
) -> EngineStages {
    let mut cold = Vec::with_capacity(ITERATIONS);
    let mut resident = Vec::with_capacity(ITERATIONS);
    let mut cold_build = Vec::with_capacity(ITERATIONS);
    let mut cold_probe = Vec::with_capacity(ITERATIONS);
    let mut cold_open = Vec::with_capacity(ITERATIONS);
    let mut resident_build = Vec::with_capacity(ITERATIONS);
    let mut resident_probe = Vec::with_capacity(ITERATIONS);
    let mut resident_open = Vec::with_capacity(ITERATIONS);
    let mut cold_rows = (0, 0);
    let mut resident_rows = (0, 0);
    for _ in 0..ITERATIONS {
        let over_file = columnar_engine::execute_on(plan, source, options).expect("the query runs");
        check(&over_file);
        cold.push(stage_of(&over_file));
        cold_rows = stage_rows_of(&over_file);
        if let Some(parts) = parts_of(&over_file) {
            cold_build.push(parts.build);
            cold_probe.push(parts.probe);
            cold_open.push(parts.open);
        }

        let over_memory = columnar_engine::execute(plan, dataset, options).expect("the query runs");
        check(&over_memory);
        resident.push(stage_of(&over_memory));
        resident_rows = stage_rows_of(&over_memory);
        if let Some(parts) = parts_of(&over_memory) {
            resident_build.push(parts.build);
            resident_probe.push(parts.probe);
            resident_open.push(parts.open);
        }
    }
    EngineStages {
        cold: PathMeasurements {
            stage: cold,
            build: cold_build,
            probe: cold_probe,
            open: cold_open,
        },
        resident: PathMeasurements {
            stage: resident,
            build: resident_build,
            probe: resident_probe,
            open: resident_open,
        },
        cold_rows,
        resident_rows,
    }
}

/// The build side's own predicates, pushed into its reader.
///
/// The build side's read is the engine's own read, so the probe pushes the
/// same predicates into the reader the engine pushes them into: the reader
/// drops the rows the predicates reject, and the dataset holds the survivors.
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

/// The build side's key column, read through the engine's own reader.
fn build_read(
    source: &ParquetSource,
    key_index: usize,
    pushdown: &PushdownPlan,
    flavor: columnar_kernels::Flavor,
) -> Result<(Timing, Dataset), Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut last: Option<Dataset> = None;
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let dataset = source.read(&[key_index], pushdown, flavor, 1)?;
        let elapsed = started.elapsed();
        samples.push(elapsed);
        last = Some(dataset);
    }
    let dataset = last.ok_or("the build side was not read")?;
    Ok((Timing::of(&samples), dataset))
}

/// The build side's keys: the per-row walk that collects them.
///
/// It is the engine's own walk -- a row at a time, out of the column's own
/// values -- and the table is built from what it collects.
fn build_keys(chunk: &ColumnChunk) -> (Timing, Vec<i64>) {
    let keys = KeyColumn::from_chunk(chunk);
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut last = Vec::new();
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let mut integers = Vec::new();
        for row in 0..chunk.len {
            let Some(KeyValue::Int(value)) = keys.value_at(row) else {
                continue;
            };
            integers.push(value);
        }
        let elapsed = started.elapsed();
        samples.push(elapsed);
        last = integers;
    }
    (Timing::of(&samples), last)
}

/// The build side's whole: the read, the keys and the table together.
///
/// The table's own cost is the difference between this and the read and the
/// keys -- the same way the sort's probe was measured.
fn build_whole(
    source: &ParquetSource,
    spec: &JoinSpec,
    flavor: columnar_kernels::Flavor,
) -> Result<Timing, Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let table = build_table(source, spec, flavor, 1)?;
        let elapsed = started.elapsed();
        std::hint::black_box(&table);
        samples.push(elapsed);
    }
    Ok(Timing::of(&samples))
}

/// The build side's whole, over a file the process has not read yet.
///
/// It is **one** read, and it is the process's first: the file's state is
/// worth what the cold read costs over the warm one, and the cold read can
/// only be taken once. It is measured **before** anything else reads the
/// build side's data -- the engine's own executions included -- so that it is
/// the cold one it claims to be.
fn build_cold_whole(
    spec: &JoinSpec,
    flavor: columnar_kernels::Flavor,
) -> Result<Duration, Box<dyn std::error::Error>> {
    let source = ParquetSource::open(&spec.build_path)?;
    let started = Instant::now();
    let table = build_table(&source, spec, flavor, 1)?;
    let elapsed = started.elapsed();
    std::hint::black_box(&table);
    Ok(elapsed)
}

/// The probe over the engine's own column, in memory.
///
/// This is the **resident** path's probe: the column is the engine's own, and
/// the rows are the dataset's own row numbers.
fn probe_over_column(
    table: &columnar_engine::operators::join::BuildTable,
    source: &ParquetSource,
    key_index: usize,
    flavor: columnar_kernels::Flavor,
) -> Result<(Timing, Vec<u32>), Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut last = Vec::new();
    for _ in 0..ITERATIONS {
        let materialised = source.read(&[key_index], &PushdownPlan::default(), flavor, 1)?;
        let chunk = materialised.chunk(PROBE_KEY).ok_or("the probe key is there")?;
        let keys = KeyColumn::from_chunk(chunk);
        let rows = 0..u32::try_from(materialised.len()).unwrap_or(u32::MAX);
        let started = Instant::now();
        let matched = probe_rows(table, &keys, rows);
        let elapsed = started.elapsed();
        samples.push(elapsed);
        last = matched;
    }
    Ok((Timing::of(&samples), last))
}

/// The probe over a column that was loaded once and then disturbed.
///
/// The query's resident probe reads the dataset's own column: it was decoded
/// when the dataset was loaded, and by the time the probe runs the build
/// side's read has been over the caches. This is the same probe over the same
/// column in that state — the column is loaded **once**, and the build side's
/// read stands between the load and the probe, exactly as it stands between
/// the dataset's load and the query's probe. The build side's read is outside
/// the clock: it is the disturbance, not the probe.
fn probe_over_held_column(
    table: &columnar_engine::operators::join::BuildTable,
    source: &ParquetSource,
    key_index: usize,
    flavor: columnar_kernels::Flavor,
    build_path: &Path,
) -> Result<(Timing, Vec<u32>), Box<dyn std::error::Error>> {
    let materialised = source.read(&[key_index], &PushdownPlan::default(), flavor, 1)?;
    let chunk = materialised.chunk(PROBE_KEY).ok_or("the probe key is there")?;
    let keys = KeyColumn::from_chunk(chunk);
    let rows = 0..u32::try_from(materialised.len()).unwrap_or(u32::MAX);

    let build_source = ParquetSource::open(build_path)?;
    let build_key_index =
        build_source.column_index(BUILD_KEY).ok_or_else(|| format!("no {BUILD_KEY} column"))?;
    let pushdown = build_pushdown(&build_source);

    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut last = Vec::new();
    for _ in 0..ITERATIONS {
        let disturbed = build_source.read(&[build_key_index], &pushdown, flavor, 1)?;
        std::hint::black_box(&disturbed);
        let started = Instant::now();
        let matched = probe_rows(table, &keys, rows.clone());
        let elapsed = started.elapsed();
        samples.push(elapsed);
        last = matched;
    }
    Ok((Timing::of(&samples), last))
}

/// The probe inside the reader's own batch loop.
///
/// This is the **cold** path's probe: the reader's own Arrow arrays, over the
/// reader's own batches, against the caches the decode is using. The decode
/// is the scan's own work, so only the probe is timed.
///
/// `offset` says whether the batch's own row numbers are made into the
/// dataset's row numbers, which is the work the engine's own cold path does
/// **outside** its probe's clock. The two differ by exactly that, and the
/// difference is what the offset costs.
fn probe_in_batches(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
    table: &columnar_engine::operators::join::BuildTable,
    offset: bool,
) -> Result<(Timing, Vec<u32>), Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut last = Vec::new();
    for _ in 0..ITERATIONS {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
        let reader = builder.with_projection(mask).with_batch_size(batch_rows).build()?;

        let mut matched = Vec::new();
        let mut base = 0_u32;
        let mut elapsed = Duration::ZERO;
        for batch in reader {
            let batch = batch?;
            let rows = u32::try_from(batch.num_rows()).unwrap_or(u32::MAX);
            let started = Instant::now();
            let array = batch.column(0);
            let Some(keys) = ArrowKeys::new(array.as_ref()) else {
                return Err("the probe's key column is a type the probe cannot read".into());
            };
            // The rows the probe is handed are the **batch's own**: the
            // reader's mask is over the batch, and the batch's rows are
            // numbered from zero. The engine's own output rows are the
            // dataset's row numbers, so the offset is added here -- and it is
            // inside the clock, because this measurement is the one that has
            // to carry it. The engine adds it in `keep_matching_rows`,
            // **outside** its own probe's clock, and the difference between
            // the two measurements is what the offset costs.
            let found = probe_rows(table, &keys, 0..rows);
            if offset {
                matched.extend(found.iter().map(|row| row.saturating_add(base)));
            } else {
                matched.extend(found.iter().copied());
            }
            elapsed += started.elapsed();
            base = base.saturating_add(rows);
        }
        samples.push(elapsed);
        last = matched;
    }
    Ok((Timing::of(&samples), last))
}

/// The plain read of a column: the floor as it was published.
///
/// It is a sum, and the compiler turns it into a loop that counts the values
/// one at a time -- which is the point: it is the fastest way to read the
/// column **a row at a time**, and the probe cannot be faster than that.
fn read_plain(values: &[i64]) -> i64 {
    let mut total = 0_i64;
    for value in values {
        total += *value;
    }
    std::hint::black_box(&total);
    total
}

/// The floor the probe cannot go below: the table's own load, one a probe row.
///
/// It is the lookup without anything else -- no output rows, no `Option`, no
/// enum -- and it is what a probe over a direct-addressed table cannot be
/// faster than.
fn floor_probe(counts: &[u8], base: i64, values: &[i64], rows: std::ops::Range<u32>) -> u64 {
    let mut total = 0_u64;
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        total += u64::from(counts.get(offset).copied().unwrap_or(0));
    }
    std::hint::black_box(&total);
    total
}

/// The probe's own table, and its own lookup over the probe's rows.
///
/// It is the engine's own table and the engine's own lookup, written out
/// here: the candidate the engine's probe is measured against, and the
/// cross-check that the engine's answer is the answer this one gives.
fn candidate_probe(
    counts: &[u8],
    base: i64,
    values: &[i64],
    rows: std::ops::Range<u32>,
) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        let count = counts.get(offset).copied().unwrap_or(0);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The candidate, with the overflow map's consult in it.
///
/// The same lookup, written out by hand, with the **overflow map's consult**
/// in it -- the map the engine's own table consults once a probe row, and the
/// map that holds **nothing** for this data, because every key's count fits in
/// its byte.
///
/// The difference between this and the plain candidate is what the consult
/// costs, **measured rather than assumed**: nothing else is different between
/// them. The map's value is a byte here, so that the count's width is the
/// plain candidate's width too; on an empty map the consult reads no value at
/// all, so the value's width is not part of what it costs.
fn candidate_with_map(
    counts: &[u8],
    base: i64,
    overflow: &HashMap<i64, u8>,
    values: &[i64],
    rows: std::ops::Range<u32>,
) -> Vec<u32> {
    let mut out = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        let byte = counts.get(offset).copied().unwrap_or(0);
        let extra = overflow.get(&key).copied().unwrap_or(0);
        let count = byte.saturating_add(extra);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// C1 — the chain: the byte read folded through `map_or` instead of `unwrap_or`.
///
/// The engine's byte read is `map_or(0, u32::from)`; the plain candidate's is
/// `unwrap_or(0)`. This rung is the fold **without** the width, so that the
/// width can be priced on its own.
fn candidate_chain(
    counts: &[u8],
    base: i64,
    values: &[i64],
    rows: std::ops::Range<u32>,
) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        let count = counts.get(offset).copied().map_or(0, |byte| byte);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// C2 — the width: the byte widened to `u32`, the way the engine's is.
///
/// The engine's lookup answers in `u32`, and the count it answers with is the
/// number of rows the key carries -- so the width is the width of the **output
/// loop** as well as of the read.
fn candidate_width(
    counts: &[u8],
    base: i64,
    values: &[i64],
    rows: std::ops::Range<u32>,
) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        let count = counts.get(offset).copied().map_or(0, u32::from);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The table the state rung reads through: the counts and the base, in a struct.
///
/// The engine's own table holds its counts in a `Vec` and its base beside it,
/// and the lookup reaches both through `&self`. This is that shape, with
/// nothing else in it.
struct CandidateTable {
    /// The count a key accounts for, one byte a key.
    counts: Vec<u8>,
    /// The smallest key.
    base: i64,
}

impl CandidateTable {
    /// The engine's byte read, and nothing else.
    fn count(&self, key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        self.counts.get(index).copied().map_or(0, u32::from)
    }
}

/// C3 — the state over `&self`: the same read, but the table is a struct and
/// the count comes from a method over `&self`.
fn candidate_state(table: &CandidateTable, values: &[i64], rows: std::ops::Range<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let count = table.count(key);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The table the map's-option rung reads through: the counts, the base and the
/// overflow map, in a struct -- the engine's own shape.
struct CandidateTableWithMap {
    /// The count a key accounts for, one byte a key.
    counts: Vec<u8>,
    /// The smallest key.
    base: i64,
    /// The keys more than 255 build rows carry. **`None`** when nothing
    /// overflowed, which is what the engine's own table holds for this data.
    overflow: Option<HashMap<i64, u32>>,
}

impl CandidateTableWithMap {
    /// The engine's own lookup, exactly: the byte, and the map only when the
    /// map holds something.
    fn count(&self, key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        let byte = self.counts.get(index).copied().map_or(0, u32::from);
        let Some(overflow) = &self.overflow else {
            return byte;
        };
        let extra = overflow.get(&key).copied().unwrap_or(0);
        byte.saturating_add(extra)
    }
}

/// C4 — the map's option: the same read, with the overflow map's `Option` read
/// and branched on, the way the engine's fast path does.
fn candidate_map_option(
    table: &CandidateTableWithMap,
    values: &[i64],
    rows: std::ops::Range<u32>,
) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let count = table.count(key);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The instrument's own key source: a plain slice of integers.
///
/// The engine's fast path reads the key column through a `KeySource`, and the
/// candidate reads a `Vec<i64>`. This is the source the engine's own lookup is
/// measured over, so that the **key source** is a difference in the ladder and
/// not a part of the lookup's.
struct PlainKeys<'a> {
    /// The key column's own values.
    values: &'a [i64],
}

impl KeySource for PlainKeys<'_> {
    fn len(&self) -> usize {
        self.values.len()
    }

    fn has_nulls(&self) -> bool {
        false
    }

    fn integer_values(&self) -> Option<&[i64]> {
        Some(self.values)
    }

    fn value_at_in_range(&self, row: usize) -> Option<KeyValue<'_>> {
        self.values.get(row).map(|value| KeyValue::Int(*value))
    }
}

/// C5 — the engine's own lookup, over the instrument's own key source.
///
/// The engine's `probe_rows` is public and takes a `KeySource`, so the engine's
/// own lookup can be measured over a plain slice without the engine giving
/// anything up.
fn engine_lookup_plain(table: &BuildTable, values: &[i64], rows: std::ops::Range<u32>) -> Vec<u32> {
    probe_rows(table, &PlainKeys { values }, rows)
}

/// The candidate inside the reader's own batch loop.
///
/// The same hand-written lookup, over the reader's own Arrow batches, so that
/// the **cold** path's remainder is measurable at all: the cold probe runs
/// inside the reader, against the caches the decode is using, and the plain
/// candidate is measured over a column in memory. The two are not the same
/// measurement, and the difference between them is the cold path's extra cost.
fn cold_candidate(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
    counts: &[u8],
    base: i64,
) -> Result<(Timing, Vec<u32>), Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut last = Vec::new();
    for _ in 0..ITERATIONS {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
        let reader = builder.with_projection(mask).with_batch_size(batch_rows).build()?;

        let mut matched = Vec::new();
        let mut offset = 0_u32;
        let mut elapsed = Duration::ZERO;
        for batch in reader {
            let batch = batch?;
            let rows = u32::try_from(batch.num_rows()).unwrap_or(u32::MAX);
            let started = Instant::now();
            let array = batch.column(0);
            let Some(values) = array.as_any().downcast_ref::<Int64Array>() else {
                return Err("the probe's key column is not a plain integer".into());
            };
            let found = candidate_probe(counts, base, values.values(), 0..rows);
            matched.extend(found.iter().map(|row| row.saturating_add(offset)));
            elapsed += started.elapsed();
            offset = offset.saturating_add(rows);
        }
        samples.push(elapsed);
        last = matched;
    }
    Ok((Timing::of(&samples), last))
}

/// The probe's own table: one byte a count, over the key's own range.
///
/// It is the engine's own table's shape, written out here, and its own
/// construction is measured directly: the table is built inside the engine's
/// build side, and the difference between the whole and its parts is smaller
/// than the noise of the read it sits in.
fn candidate_table(keys: &[i64], base: i64, width: usize) -> Timing {
    repeat(ITERATIONS, || {
        let started = Instant::now();
        let counts = candidate_counts(keys, base, width);
        let elapsed = started.elapsed();
        std::hint::black_box(&counts);
        elapsed
    })
}

/// The candidate's table: one byte a count, over the key's own range.
///
/// It is the engine's own table's shape, written out here, so that the floor
/// is a floor over the table the engine actually builds.
fn candidate_counts(keys: &[i64], base: i64, width: usize) -> Vec<u8> {
    let mut counts = vec![0_u8; width];
    for key in keys {
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        let Some(count) = counts.get_mut(offset) else {
            continue;
        };
        if let Some(next) = count.checked_add(1) {
            *count = next;
        }
    }
    counts
}

/// The build side's file, beside the probe side's.
///
/// It is the generator's own rule: the file named `lineitem` is joined by the
/// file named `orders`, beside it. The rule belongs to the generator, and the
/// probe mirrors it rather than repeating it in the engine.
fn build_path_for(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let orders_stem = match stem.strip_prefix("lineitem") {
        Some(rest) => format!("orders{rest}"),
        None => "orders".to_owned(),
    };
    path.with_file_name(format!("{orders_stem}.parquet"))
}

/// The probe's arguments: the file and where the report goes.
fn arguments() -> Result<(PathBuf, PathBuf, String), Box<dyn std::error::Error>> {
    let mut path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut code: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--data" => path = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--code" => code = args.next(),
            "--help" | "-h" => {
                println!(
                    "usage: join_cost_probe --data <parquet> [--out <report.json>] [--code <name>]"
                );
                std::process::exit(0);
            }
            // The path may also be given on its own, the way the other probes
            // take it.
            other if !other.starts_with("--") => path = Some(PathBuf::from(other)),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let path = path.ok_or("--data is required")?;
    let out = out.unwrap_or_else(|| PathBuf::from("join-cost.json"));
    // The code the campaign measured. The commit and the tree's state are the
    // fallback, and they are carried in the report beside it either way.
    let code = code.unwrap_or_else(|| format!("{} ({})", commit(), tree_state()));
    Ok((path, out, code))
}

/// The build side's own parts.
struct BuildParts {
    /// Opening the build side's file.
    open: Timing,
    /// Reading it.
    read: Timing,
    /// The per-row walk that collects its keys.
    keys: Timing,
    /// The whole of the build side.
    whole: Timing,
    /// The table's own construction.
    table: Timing,
    /// The keys the build side collected.
    collected: Vec<i64>,
    /// The table's base: the smallest key it addresses.
    base: i64,
    /// The table's width: the keys it addresses.
    width: usize,
}

/// The probe's own parts.
struct ProbeParts {
    /// The probe over the engine's own column: the resident path's probe.
    column: Timing,
    /// The probe inside the reader's batch loop: the cold path's probe.
    batches: Timing,
    /// The same probe without the row-number offset.
    batches_no_offset: Timing,
    /// The probe over a column that was loaded once and then disturbed.
    held: Timing,
    /// The floor the probe cannot go below.
    floor: Timing,
    /// The probe's own candidate.
    candidate: Timing,
    /// The candidate with the overflow map's consult in it.
    with_map: Timing,
    /// The candidate inside the reader's own batch loop.
    cold: Timing,
    /// The candidate whose byte read is folded through `map_or`.
    chain: Timing,
    /// The candidate whose byte is widened to `u32`.
    width: Timing,
    /// The candidate that reads its table through `&self`.
    state: Timing,
    /// The candidate that reads the overflow map's `Option`.
    map_option: Timing,
    /// The engine's own lookup over the instrument's own key source.
    engine_plain: Timing,
    /// The rows the engine's probe produced.
    engine_rows: Vec<u32>,
    /// The probe key column's own values.
    values: Vec<i64>,
}

/// Everything the probe measured.
struct Measured {
    /// The name of the code this campaign measured.
    code: String,
    /// The timings, in the order the report carries them.
    parts: Vec<(&'static str, Timing)>,
    /// The cold path's own measurements, per iteration.
    cold: PathMeasurements,
    /// The resident path's own measurements, per iteration.
    resident: PathMeasurements,
    /// The rows the join's stage reports, on the cold path.
    cold_rows: (u64, u64),
    /// The rows the join's stage reports, on the resident path.
    resident_rows: (u64, u64),
    /// The rows the join produced.
    engine_rows: usize,
    /// The rows the table holds.
    table_rows: usize,
    /// The rows the file holds.
    rows: usize,
}

/// The build side's parts, measured on their own.
fn build_parts(
    spec: &JoinSpec,
    flavor: columnar_kernels::Flavor,
) -> Result<BuildParts, Box<dyn std::error::Error>> {
    let build_source = ParquetSource::open(&spec.build_path)?;
    let build_key_index =
        build_source.column_index(BUILD_KEY).ok_or_else(|| format!("no {BUILD_KEY} column"))?;
    let pushdown = build_pushdown(&build_source);

    let open = repeat(ITERATIONS, || {
        let started = Instant::now();
        let opened = ParquetSource::open(&spec.build_path).expect("the build side opens");
        let elapsed = started.elapsed();
        std::hint::black_box(&opened);
        elapsed
    });
    // The build side's whole, measured **after** the file has been read: this
    // is over a file the process has already read, and the one the query's
    // build side is measured against. The one over a file it has not read is
    // `build_cold_whole`, and it is taken before anything else reads it.
    let (read, dataset) = build_read(&build_source, build_key_index, &pushdown, flavor)?;
    let chunk = dataset.chunk(BUILD_KEY).ok_or("the build side's key is there")?;
    let (keys, collected) = build_keys(chunk);
    let whole = build_whole(&build_source, spec, flavor)?;
    let (base, width) = build_source.column_range(BUILD_KEY).map_or((0_i64, 0_usize), |range| {
        let (ScalarValue::Int64(min), ScalarValue::Int64(max)) = range else {
            return (0_i64, 0_usize);
        };
        (min, usize::try_from(max.saturating_sub(min).saturating_add(1)).unwrap_or(0))
    });
    let table = candidate_table(&collected, base, width);
    Ok(BuildParts { open, read, keys, whole, table, collected, base, width })
}

/// The probe's parts, measured on their own.
fn probe_parts(
    path: &Path,
    source: &ParquetSource,
    table: &columnar_engine::operators::join::BuildTable,
    build: &BuildParts,
    spec: &JoinSpec,
    flavor: columnar_kernels::Flavor,
) -> Result<ProbeParts, Box<dyn std::error::Error>> {
    let probe_index =
        source.column_index(PROBE_KEY).ok_or_else(|| format!("no {PROBE_KEY} column"))?;
    let (column, engine_rows) = probe_over_column(table, source, probe_index, flavor)?;
    let batch_rows = source.batch_rows_under(MAX_READER_BATCH_ROWS);
    let value_index = source.column_index("l_extendedprice").unwrap_or(probe_index);
    let (batches, batched_rows) =
        probe_in_batches(path, &[probe_index, value_index], batch_rows, table, true)?;
    // The same probe without the row-number offset: the batch's own row
    // numbers are not made into the dataset's. The engine's own cold path does
    // that arithmetic **outside** its probe's clock, so the difference between
    // these two is what the offset costs.
    let (batches_no_offset, _) =
        probe_in_batches(path, &[probe_index, value_index], batch_rows, table, false)?;
    // The same probe over a column that was loaded once and then disturbed by
    // the build side's read: the state the query's resident probe is in.
    let (held, held_rows) =
        probe_over_held_column(table, source, probe_index, flavor, &spec.build_path)?;
    assert_eq!(held_rows, engine_rows, "the held column probes differently");

    let values = probe_values(source, probe_index, flavor)?;
    let counts = candidate_counts(&build.collected, build.base, build.width);
    let all_rows = 0..u32::try_from(source.rows()).unwrap_or(u32::MAX);
    let candidate_rows = candidate_probe(&counts, build.base, &values, all_rows.clone());
    assert_eq!(candidate_rows, engine_rows, "the candidate probes differently");
    assert_eq!(candidate_rows, batched_rows, "the two paths probe differently");

    let floor = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let total = floor_probe(&counts, build.base, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    })?;
    let candidate = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = candidate_probe(&counts, build.base, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    // The candidate with the overflow map's consult in it. The map is the
    // engine's own map's **kind** -- the same type, and empty, because every
    // key's count fits in its byte for this data -- so the consult costs what
    // the engine's consult costs.
    let overflow: HashMap<i64, u8> = HashMap::new();
    let with_map_rows =
        candidate_with_map(&counts, build.base, &overflow, &values, all_rows.clone());
    assert_eq!(with_map_rows, candidate_rows, "the map changes no row");
    let with_map = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = candidate_with_map(&counts, build.base, &overflow, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    // The cold candidate: the same hand-written lookup, inside the reader's
    // own batch loop, so that the cold path's remainder is measurable.
    let (cold, cold_rows) =
        cold_candidate(path, &[probe_index, value_index], batch_rows, &counts, build.base)?;
    assert_eq!(cold_rows, engine_rows, "the cold candidate probes differently");

    // The ladder: the remainder's parts, one at a time.
    let ladder = ladder_parts(&LadderInput {
        source,
        probe_index,
        flavor,
        counts: &counts,
        base: build.base,
        values: &values,
        all_rows: all_rows.clone(),
        table,
        engine_rows: &engine_rows,
    })?;

    Ok(ProbeParts {
        column,
        batches,
        batches_no_offset,
        held,
        floor,
        candidate,
        with_map,
        cold,
        chain: ladder.chain,
        width: ladder.width,
        state: ladder.state,
        map_option: ladder.map_option,
        engine_plain: ladder.engine_plain,
        engine_rows,
        values,
    })
}

/// What the ladder is measured over.
struct LadderInput<'a> {
    /// The probe side's source.
    source: &'a ParquetSource,
    /// The probe key's column index.
    probe_index: usize,
    /// The kernel flavour.
    flavor: columnar_kernels::Flavor,
    /// The table's counts, one byte a key.
    counts: &'a [u8],
    /// The table's base: the smallest key it addresses.
    base: i64,
    /// The values the rows are checked over.
    values: &'a [i64],
    /// The rows the probe covers.
    all_rows: std::ops::Range<u32>,
    /// The engine's own table.
    table: &'a BuildTable,
    /// The rows the engine's probe produced.
    engine_rows: &'a [u32],
}

/// The ladder's rungs, measured.
struct LadderParts {
    /// The candidate whose byte read is folded through `map_or`.
    chain: Timing,
    /// The candidate whose byte is widened to `u32`.
    width: Timing,
    /// The candidate that reads its table through `&self`.
    state: Timing,
    /// The candidate that reads the overflow map's `Option`.
    map_option: Timing,
    /// The engine's own lookup over the instrument's own key source.
    engine_plain: Timing,
}

/// The ladder: the remainder's price, one thing at a time.
///
/// Each rung is the plain candidate plus **exactly one** thing, over the same
/// rows and the same table, and each one is checked against the engine's rows
/// before it is timed.
fn ladder_parts(input: &LadderInput<'_>) -> Result<LadderParts, Box<dyn std::error::Error>> {
    let source = input.source;
    let probe_index = input.probe_index;
    let flavor = input.flavor;
    let counts = input.counts;
    let base = input.base;
    let values = input.values;
    let all_rows = input.all_rows.clone();
    let table = input.table;
    let engine_rows = input.engine_rows;
    // The rows are checked **before** anything is timed: a rung that answers
    // differently is not a rung, it is a different join. The check runs over
    // the values the instrument keeps; the timing runs over a column
    // materialised afresh, which holds the same numbers.
    let chain_rows = candidate_chain(counts, base, values, all_rows.clone());
    assert_eq!(chain_rows, engine_rows, "the chain probes differently");
    let width_rows = candidate_width(counts, base, values, all_rows.clone());
    assert_eq!(width_rows, engine_rows, "the width probes differently");
    let state_table = CandidateTable { counts: counts.to_vec(), base };
    let state_rows = candidate_state(&state_table, values, all_rows.clone());
    assert_eq!(state_rows, engine_rows, "the state probes differently");
    let map_table = CandidateTableWithMap { counts: counts.to_vec(), base, overflow: None };
    let map_rows = candidate_map_option(&map_table, values, all_rows.clone());
    assert_eq!(map_rows, engine_rows, "the map's option probes differently");
    let engine_plain_rows = engine_lookup_plain(table, values, all_rows.clone());
    assert_eq!(engine_plain_rows, engine_rows, "the engine's lookup probes differently");

    let chain = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = candidate_chain(counts, base, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    let width = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = candidate_width(counts, base, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    let state = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = candidate_state(&state_table, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    let map_option = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = candidate_map_option(&map_table, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    let engine_plain = over_fresh_column(source, probe_index, flavor, |values| {
        let started = Instant::now();
        let rows = engine_lookup_plain(table, values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    })?;

    Ok(LadderParts { chain, width, state, map_option, engine_plain })
}

/// The probe key column's own values, as a plain slice of integers.
fn plain_values(chunk: &ColumnChunk) -> Result<&[i64], Box<dyn std::error::Error>> {
    match &chunk.encoding {
        columnar_core::Encoding::Int64 { values } => Ok(values),
        other => Err(format!("the probe's key is not a plain integer: {other:?}").into()),
    }
}

/// Times a measurement over the probe key column, **materialised afresh** each
/// time.
///
/// The engine's own query materialises the column and then probes it, so the
/// column it probes is one the reader has just written. A candidate that read a
/// buffer kept warm across the iterations would be measured on a **cheaper**
/// column than the engine pays for, and the difference between the two would
/// be the buffer's, not the lookup's.
fn over_fresh_column<F>(
    source: &ParquetSource,
    key_index: usize,
    flavor: columnar_kernels::Flavor,
    mut measure: F,
) -> Result<Timing, Box<dyn std::error::Error>>
where
    F: FnMut(&[i64]) -> Duration,
{
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let materialised = source.read(&[key_index], &PushdownPlan::default(), flavor, 1)?;
        let chunk = materialised.chunk(PROBE_KEY).ok_or("the probe key is there")?;
        let values = plain_values(chunk)?;
        samples.push(measure(values));
    }
    Ok(Timing::of(&samples))
}

/// The probe key column's own values, as a plain slice of integers.
fn probe_values(
    source: &ParquetSource,
    probe_index: usize,
    flavor: columnar_kernels::Flavor,
) -> Result<Vec<i64>, Box<dyn std::error::Error>> {
    let materialised = source.read(&[probe_index], &PushdownPlan::default(), flavor, 1)?;
    let chunk = materialised.chunk(PROBE_KEY).ok_or("the probe key is there")?;
    match &chunk.encoding {
        columnar_core::Encoding::Int64 { values } => Ok(values.clone()),
        other => Err(format!("the probe's key is not a plain integer: {other:?}").into()),
    }
}

/// Measures everything the probe measures.
fn measure(path: &Path, code: &str) -> Result<Measured, Box<dyn std::error::Error>> {
    let flavor = columnar_kernels::best();
    let options = ExecOptions::default();
    let source = ParquetSource::open(path)?;
    let rows = source.rows();

    let plan = (definition().plan)(&build_path_for(path));
    let dataset = Dataset::read_parquet(path)?;
    let spec = plan.join.as_ref().ok_or("the plan has a join")?;

    // The build side's file state is measured **first**: it is the process's
    // first read of the build side's data, and every later read is over a
    // file the process has already read. It is taken before the engine's own
    // executions, so that the query's build side and the standalone's are
    // measured over the **same** state of the file -- a warm one -- and the
    // file's state does not enter the build side's gap.
    let build_cold = build_cold_whole(spec, flavor)?;

    let stages = engine_stages(&plan, &source, &dataset, &options);

    let build = build_parts(spec, flavor)?;
    let table = build_table(&ParquetSource::open(&spec.build_path)?, spec, flavor, 1)?;
    let probe = probe_parts(path, &source, &table, &build, spec, flavor)?;

    // The controls: the plain read of the probe key column, and the same read
    // over a buffer small enough to be in the caches.
    let small: Vec<i64> =
        (0..SMALL_KEYS).map(|index| i64::try_from(index % 97).unwrap_or(0)).collect();
    let read_plain_timing = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = read_plain(&probe.values);
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });
    let read_small = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = read_plain(&small);
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });

    let mut parts = vec![
        ("stage_cold", stages.cold.stage_timing()),
        ("stage_resident", stages.resident.stage_timing()),
        ("build_open", build.open),
        ("build_read", build.read),
        ("build_keys", build.keys),
        ("build_whole", build.whole),
        ("build_whole_cold", Timing::of(&[build_cold])),
        ("table_own", build.table),
        ("probe_column", probe.column),
        ("probe_batches", probe.batches),
        ("probe_batches_no_offset", probe.batches_no_offset),
        ("probe_held", probe.held),
        ("floor_probe", probe.floor),
        ("candidate_probe", probe.candidate),
        ("candidate_with_map", probe.with_map),
        ("cold_candidate", probe.cold),
        ("candidate_chain", probe.chain),
        ("candidate_width", probe.width),
        ("candidate_state", probe.state),
        ("candidate_map_option", probe.map_option),
        ("engine_lookup_plain", probe.engine_plain),
        ("read_plain", read_plain_timing),
        ("read_small", read_small),
        ("clock_cost", clock_cost()),
    ];
    // The engine's own account of the stage, when the engine gives one.
    if let Some((build, probe, open)) = stages.cold.parts() {
        parts.push(("engine_build_cold", build));
        parts.push(("engine_probe_cold", probe));
        parts.push(("engine_open_cold", open));
    }
    if let Some((build, probe, open)) = stages.resident.parts() {
        parts.push(("engine_build_resident", build));
        parts.push(("engine_probe_resident", probe));
        parts.push(("engine_open_resident", open));
    }
    Ok(Measured {
        code: code.to_owned(),
        parts,
        cold: stages.cold,
        resident: stages.resident,
        cold_rows: stages.cold_rows,
        resident_rows: stages.resident_rows,
        engine_rows: probe.engine_rows.len(),
        table_rows: table.rows(),
        rows,
    })
}

/// The context's decomposition: the gaps the engine's own account of the
/// stage makes, and the leaf each gap is attributed to.
#[derive(Default)]
struct Context {
    /// The build side's gap: the query's build side against the standalone one.
    build: Option<f64>,
    /// The open's gap: the query's open against the standalone one.
    open: Option<f64>,
    /// The probe's gap: the query's probe against the standalone one.
    probe: Option<f64>,
    /// The three gaps together: the context, by another route.
    sum: Option<f64>,
    /// The context: the residual less the standalone probe of the same work.
    ///
    /// It is the total the parts sum to.
    total: Option<f64>,
    /// The stage's own residual: what the stage leaves after the build side's
    /// whole and its open.
    residual: Option<f64>,
    /// The build side's file state: the first read against a later one.
    build_file_state: f64,
    /// The build side's gap, less its file state: the build side's own.
    build_residual: Option<f64>,
    /// The probe's file state: the held column against the fresh one.
    probe_file_state: Option<f64>,
    /// The probe's row-number offset: the work the standalone carries and the
    /// query's clock does not.
    ///
    /// It is the offset's **cost**, and it enters the probe's gap **against**
    /// it: the standalone's clock carries the offset and the query's does not.
    probe_row_offset: Option<f64>,
    /// The part of the context that lies in no gap.
    ///
    /// It is zero when the ladder closes, and it is published so that the
    /// claim is checked rather than asserted.
    host: Option<f64>,
    /// The probe's gap, less its file state and its offset: the probe's own.
    probe_residual: Option<f64>,
    /// The ladder's own reconciliation: the context less the parts, taken over
    /// the iterations.
    ///
    /// It is zero by construction -- the parts are the gaps, and the gaps are
    /// the stage's own parts less the standalone's -- and it is published so
    /// that the claim is checked rather than asserted.
    reconciliation: Option<f64>,
    /// The published parts against the published context.
    ///
    /// A median of a sum is not the sum of the medians, so the parts' medians
    /// do not add up to the context's median. The difference is **the
    /// reduction's own noise**, and it is published rather than folded away.
    reduction_noise: Option<f64>,
    /// The engine's own parts against the stage's elapsed, per iteration.
    stage_parts_gap: Option<f64>,
}

/// What one path's context is measured over.
struct ContextInput<'a> {
    /// The engine's own measurements, per iteration.
    path: &'a PathMeasurements,
    /// The build side's whole, measured outside the query.
    build_whole: f64,
    /// The build side's open, measured outside the query.
    build_open: f64,
    /// The probe, measured outside the query.
    standalone_probe: f64,
    /// The build side's file state: the first read against a later one.
    build_file_state: f64,
    /// The probe's file state, on the paths that have one.
    probe_file_state: Option<f64>,
    /// The probe's row-number offset, on the paths that have one.
    probe_row_offset: Option<f64>,
}

/// The context's decomposition, for one path.
///
/// The gaps come from the engine's **own** account of the stage, so they are
/// not inferred by subtracting a measurement taken in another context, and the
/// three of them are the context by another route.
///
/// The ladder is built **per iteration**, because the stage's elapsed is the
/// three parts **added together** in every iteration and a median of a sum is
/// not the sum of the medians. Every iteration's parts sum to that
/// iteration's context exactly, and the reconciliation -- published — is the
/// median of the difference over the iterations, which is zero.
///
/// Each gap is then attributed to the leaf that can explain it, and the gap's
/// **residual** is what is left of it unexplained. The leaves are **bounds**:
/// the build side's file state is the most the file's state can explain of the
/// build side's gap, because the query reads the build side's file once per
/// execution and the published figure is the median of five of them — so the
/// file's state at the time of the measurement is somewhere between a cold read
/// and a warm one, and the bound is what the cold one costs over the warm one.
fn context_ladder(input: &ContextInput<'_>) -> Context {
    if input.path.build.is_empty() {
        return Context::default();
    }
    let file_state = input.probe_file_state.unwrap_or(0.0);
    let offset = input.probe_row_offset.unwrap_or(0.0);
    let mut build_gaps = Vec::new();
    let mut open_gaps = Vec::new();
    let mut probe_gaps = Vec::new();
    let mut probe_residuals = Vec::new();
    let mut contexts = Vec::new();
    let mut reconciliations = Vec::new();
    let mut host_remainders = Vec::new();
    let mut stage_gaps = Vec::new();
    let iterations =
        input.path.stage.iter().zip(&input.path.build).zip(&input.path.probe).zip(&input.path.open);
    for (((stage, build), probe), open) in iterations {
        let stage = ms(*stage);
        let build = ms(*build);
        let probe = ms(*probe);
        let open = ms(*open);
        let build_gap = build - input.build_whole;
        let open_gap = open - input.build_open;
        let probe_gap = probe - input.standalone_probe;
        let context = stage - input.build_whole - input.build_open - input.standalone_probe;
        // The offset is in the standalone's clock and not in the query's, so
        // it is **against** the probe's gap; the file's state is the query's
        // own, so it is **with** it.
        let probe_residual = probe_gap - file_state + offset;
        // The leaves: the build side's gap is the build side's own -- both
        // sides read the build side's file warm, so the file's state does not
        // enter it -- and the probe's gap is its file's state, its row-number
        // offset and what is left of it.
        let leaves = build_gap + open_gap + (file_state - offset) + probe_residual;
        build_gaps.push(build_gap);
        open_gaps.push(open_gap);
        probe_gaps.push(probe_gap);
        probe_residuals.push(probe_residual);
        contexts.push(context);
        reconciliations.push(context - leaves);
        host_remainders.push(context - (build_gap + open_gap + probe_gap));
        stage_gaps.push(stage - (build + probe + open));
    }
    let build = median_ms(&build_gaps);
    let open = median_ms(&open_gaps);
    let probe = median_ms(&probe_gaps);
    let sum = build + open + probe;
    let total = median_ms(&contexts);
    Context {
        build: Some(build),
        open: Some(open),
        probe: Some(probe),
        sum: Some(sum),
        total: Some(total),
        residual: Some(Timing::of(&input.path.stage).median - input.build_whole - input.build_open),
        build_file_state: input.build_file_state,
        build_residual: Some(build),
        probe_file_state: input.probe_file_state,
        probe_row_offset: input.probe_row_offset,
        probe_residual: Some(median_ms(&probe_residuals)),
        reconciliation: Some(median_ms(&reconciliations)),
        reduction_noise: Some(sum - total),
        host: Some(median_ms(&host_remainders)),
        stage_parts_gap: Some(median_ms(&stage_gaps)),
    }
}

/// One of the report's parts, by name.
fn part_of(measured: &Measured, name: &str) -> f64 {
    measured
        .parts
        .iter()
        .find(|(part, _)| *part == name)
        .map_or(f64::NAN, |(_, timing)| timing.median)
}

/// The two paths' contexts, and their decomposition.
///
/// The context is the stage's **own** residual less the standalone probe of
/// the same work, and it is decomposed into the three gaps the engine's own
/// account of the stage makes -- which sum to it, and whose leaves say which
/// mechanism can explain each of them.
fn contexts(measured: &Measured) -> (Context, Context) {
    let part = |name: &str| part_of(measured, name);
    // The build side's file state is a property of the file and not of the
    // path, so both paths carry the same one.
    let build_file_state = part("build_whole_cold") - part("build_whole");
    let cold = context_ladder(&ContextInput {
        path: &measured.cold,
        build_whole: part("build_whole"),
        build_open: part("build_open"),
        standalone_probe: part("probe_batches"),
        build_file_state,
        // The cold path's probe reads the file inside the reader on **both**
        // sides, so the file's state is the same by construction and there is
        // no file-state leaf: the row-number offset is the bracket that
        // differs.
        probe_file_state: None,
        probe_row_offset: Some(part("probe_batches") - part("probe_batches_no_offset")),
    });
    let resident = context_ladder(&ContextInput {
        path: &measured.resident,
        build_whole: part("build_whole"),
        build_open: part("build_open"),
        standalone_probe: part("probe_column"),
        build_file_state,
        // The resident path's probe reads a column the dataset has held since
        // it was loaded, with the build side's read in between, and the
        // standalone reads its column afresh: the file's state is the leaf.
        probe_file_state: Some(part("probe_held") - part("probe_column")),
        // The resident path's probe carries no row-number offset: it probes
        // the dataset's own rows.
        probe_row_offset: None,
    });
    (cold, resident)
}

/// The report's derived figures: every difference it publishes.
fn derived_ms(measured: &Measured, cold: &Context, resident: &Context) -> Figures {
    let part = |name: &str| part_of(measured, name);
    let number = |value: Option<f64>| -> serde_json::Value {
        value.map_or(serde_json::Value::Null, serde_json::Value::from)
    };
    // The ladder's six increments. They add up to the remainder by
    // construction, so what they say is **where** the remainder is, and the
    // difference between their sum and the remainder is the noise the
    // decomposition carries.
    let chain_cost = part("candidate_chain") - part("candidate_probe");
    let width_cost = part("candidate_width") - part("candidate_chain");
    let state_cost = part("candidate_state") - part("candidate_width");
    let map_option_cost = part("candidate_map_option") - part("candidate_state");
    let loop_cost = part("engine_lookup_plain") - part("candidate_map_option");
    let key_source_cost = part("probe_column") - part("engine_lookup_plain");
    let ladder_sum =
        chain_cost + width_cost + state_cost + map_option_cost + loop_cost + key_source_cost;
    let remainder = part("probe_column") - part("candidate_probe");

    let mut derived = serde_json::Map::new();
    let mut put = |name: &str, value: serde_json::Value| {
        derived.insert(name.to_owned(), value);
    };
    put(
        "build_table",
        serde_json::json!(part("build_whole") - part("build_read") - part("build_keys")),
    );
    put("output_rows", serde_json::json!(part("candidate_probe") - part("floor_probe")));
    put("the_probe_s_remainder", serde_json::json!(remainder));
    put("the_map_s_share", serde_json::json!(part("candidate_with_map") - part("candidate_probe")));
    put("the_cold_remainder", serde_json::json!(part("probe_batches") - part("cold_candidate")));
    put("the_two_paths", serde_json::json!(part("stage_cold") - part("stage_resident")));
    put("the_two_probes", serde_json::json!(part("probe_batches") - part("probe_column")));
    put("the_chain_s_cost", serde_json::json!(chain_cost));
    put("the_width_s_cost", serde_json::json!(width_cost));
    put("the_state_s_cost", serde_json::json!(state_cost));
    put("the_map_option_s_cost", serde_json::json!(map_option_cost));
    put("the_loop_s_cost", serde_json::json!(loop_cost));
    put("the_key_source_s_cost", serde_json::json!(key_source_cost));
    put("the_ladder_s_sum", serde_json::json!(ladder_sum));
    put("the_unexplained", serde_json::json!(remainder - ladder_sum));
    put("the_residual_cold", serde_json::json!(cold.residual));
    put("the_residual_resident", serde_json::json!(resident.residual));
    put("the_context_cold", serde_json::json!(cold.total));
    put("the_context_resident", serde_json::json!(resident.total));
    put("the_build_gap_cold", number(cold.build));
    put("the_open_gap_cold", number(cold.open));
    put("the_probe_gap_cold", number(cold.probe));
    put("the_gaps_sum_cold", number(cold.sum));
    put("the_host_remainder_cold", number(cold.host));
    put("the_build_residual_cold", number(cold.build_residual));
    put("the_probe_residual_cold", number(cold.probe_residual));
    put("the_stage_parts_gap_cold", number(cold.stage_parts_gap));
    put("the_reconciliation_cold", number(cold.reconciliation));
    put("the_reduction_s_noise_cold", number(cold.reduction_noise));
    put("the_build_gap_resident", number(resident.build));
    put("the_open_gap_resident", number(resident.open));
    put("the_probe_gap_resident", number(resident.probe));
    put("the_gaps_sum_resident", number(resident.sum));
    put("the_host_remainder_resident", number(resident.host));
    put("the_build_residual_resident", number(resident.build_residual));
    put("the_probe_residual_resident", number(resident.probe_residual));
    put("the_stage_parts_gap_resident", number(resident.stage_parts_gap));
    put("the_reconciliation_resident", number(resident.reconciliation));
    put("the_reduction_s_noise_resident", number(resident.reduction_noise));
    put("the_build_file_state", serde_json::json!(cold.build_file_state));
    put("the_probe_row_offset", number(cold.probe_row_offset.map(|cost| -cost)));
    put("the_probe_file_state", number(resident.probe_file_state));
    // The resident path's added clock: two reads and one subtraction.
    put("the_added_clock_cost", serde_json::json!(part("clock_cost") * 2.0));
    derived
}

/// What each clock brackets, and what it does not include.
///
/// A figure without a stated bracket is not a measurement, and the ladder's
/// parts are only comparable once the two sides' brackets are published side
/// by side.
fn brackets() -> Figures {
    let rows = [
        ("stage_cold", "two disjoint intervals -- the build table's construction, and then the probe inside the reader -- and the build side's file open. Not the reader's decode, and not the gap between the two intervals."),
        ("stage_resident", "one interval -- the build table's construction, the probe key column's borrow, and the probe -- and the build side's file open. Not the dataset's own read, which happened before the query."),
        ("build_whole", "the build table's construction alone. Not the build side's file open, which the stage's own clock carries."),
        ("build_whole_cold", "the build table's construction over a file the process has not read yet: one read, and the process's first."),
        ("build_open", "the build side's file open, on its own."),
        ("probe_batches", "the probe inside the reader's own batch loop, plus the per-batch key column's construction, plus the row-number offset arithmetic. Not the reader's decode, which is the scan's own work."),
        ("probe_batches_no_offset", "the same probe without the row-number offset arithmetic, which the engine's own cold path does outside its probe's clock."),
        ("probe_column", "the probe over a column read from the file for that iteration. Not the column's read, which it has just done."),
        ("probe_held", "the probe over a column loaded once and disturbed by the build side's read: the state the query's resident probe is in."),
        ("engine_build_cold", "the build table's construction, in the query's process, on the cold path."),
        ("engine_probe_cold", "the probe inside the reader, in the query's process, on the cold path."),
        ("engine_open_cold", "the build side's file open, in the query's process, on the cold path."),
        ("engine_build_resident", "the build table's construction, in the query's process, on the resident path."),
        ("engine_probe_resident", "the probe over the dataset's own column, in the query's process, on the resident path."),
        ("engine_open_resident", "the build side's file open, in the query's process, on the resident path."),
    ];
    let mut map = serde_json::Map::new();
    for (name, bracket) in rows {
        map.insert(name.to_owned(), serde_json::json!(bracket));
    }
    map
}

/// The report, and the printout that goes with it.
fn report(out: &Path, path: &Path, measured: &Measured) -> Result<(), Box<dyn std::error::Error>> {
    let mut parts_ms = serde_json::Map::new();
    let mut spreads_ms = serde_json::Map::new();
    for (name, timing) in &measured.parts {
        parts_ms.insert((*name).to_owned(), serde_json::json!(timing.median));
        spreads_ms.insert((*name).to_owned(), serde_json::json!(timing.spread()));
    }
    let (cold, resident) = contexts(measured);
    let report = serde_json::json!({
        "probe": "join-cost",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit(),
        "tree": tree_state(),
        "code": measured.code,
        // Whether the engine's own plan output reported the join stage's
        // parts. It is a structural fact about the code the campaign
        // measured, and it tells the two sets apart when the commit and the
        // tree's state do not.
        "engine_reports_parts": !measured.cold.build.is_empty(),
        "host_load_average": load_average(),
        "rows": measured.rows,
        "probe_key": PROBE_KEY,
        "build_key": BUILD_KEY,
        "parts_ms": parts_ms,
        "spreads_ms": spreads_ms,
        "brackets": brackets(),
        "derived_ms": derived_ms(measured, &cold, &resident),
        "engine_rows": measured.engine_rows,
        "table_rows": measured.table_rows,
        "stage_rows": {
            "cold": [measured.cold_rows.0, measured.cold_rows.1],
            "resident": [measured.resident_rows.0, measured.resident_rows.1],
        },
    });
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(out, format!("{json}\n"))?;

    println!("\n{:<22}{:>12}", "the part", "ms");
    for (name, timing) in &measured.parts {
        println!("{name:<22}{:>12.3}", timing.median);
    }
    println!("\nwrote {}", out.display());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (path, out, code) = arguments()?;
    let measured = measure(&path, &code)?;
    report(&out, &path, &measured)
}
