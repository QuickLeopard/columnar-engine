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
//! The **table**, the **output** and the probe's **remainder** are the
//! differences: the table is what `build_whole` costs over the read and the
//! keys, the output is what the candidate probe costs over the floor, and the
//! remainder is what the engine's own probe costs over the candidate's.
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

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_core::{ColumnChunk, ComparisonOp, ScalarValue};
use columnar_engine::operators::join::{build_table, probe_rows};
use columnar_engine::operators::key::{ArrowKeys, KeyColumn, KeyValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::queries::BuiltinQuery;
use columnar_engine::{Dataset, ExecOptions, JoinSpec, ParquetSource, MAX_READER_BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;

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

/// Repeats a measurement, and returns its median and its spread.
fn repeat(iterations: usize, mut measure: impl FnMut() -> Duration) -> Timing {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        samples.push(measure());
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

/// The engine's own join stage, on both paths.
///
/// The cold path is the one where the scan feeds the join, and the resident
/// path is the one where the join probes the dataset itself.
fn engine_stages(
    plan: &columnar_engine::QueryPlan,
    source: &ParquetSource,
    dataset: &Dataset,
    options: &ExecOptions,
) -> (Timing, Timing, (u64, u64), (u64, u64)) {
    let mut cold = Vec::with_capacity(ITERATIONS);
    let mut resident = Vec::with_capacity(ITERATIONS);
    let mut cold_rows = (0, 0);
    let mut resident_rows = (0, 0);
    for _ in 0..ITERATIONS {
        let over_file = columnar_engine::execute_on(plan, source, options).expect("the query runs");
        check(&over_file);
        cold.push(stage_of(&over_file));
        cold_rows = stage_rows_of(&over_file);

        let over_memory = columnar_engine::execute(plan, dataset, options).expect("the query runs");
        check(&over_memory);
        resident.push(stage_of(&over_memory));
        resident_rows = stage_rows_of(&over_memory);
    }
    (Timing::of(&cold), Timing::of(&resident), cold_rows, resident_rows)
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

/// The probe inside the reader's own batch loop.
///
/// This is the **cold** path's probe: the reader's own Arrow arrays, over the
/// reader's own batches, against the caches the decode is using. The decode
/// is the scan's own work, so only the probe is timed.
fn probe_in_batches(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
    table: &columnar_engine::operators::join::BuildTable,
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
            // inside the clock, because the engine's cold path pays it too.
            let found = probe_rows(table, &keys, 0..rows);
            matched.extend(found.iter().map(|row| row.saturating_add(base)));
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
fn arguments() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let mut path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--data" => path = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("usage: join_cost_probe --data <parquet> [--out <report.json>]");
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
    Ok((path, out))
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
    /// The floor the probe cannot go below.
    floor: Timing,
    /// The probe's own candidate.
    candidate: Timing,
    /// The rows the engine's probe produced.
    engine_rows: Vec<u32>,
    /// The probe key column's own values.
    values: Vec<i64>,
}

/// Everything the probe measured.
struct Measured {
    /// The timings, in the order the report carries them.
    parts: Vec<(&'static str, Timing)>,
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
    flavor: columnar_kernels::Flavor,
) -> Result<ProbeParts, Box<dyn std::error::Error>> {
    let probe_index =
        source.column_index(PROBE_KEY).ok_or_else(|| format!("no {PROBE_KEY} column"))?;
    let (column, engine_rows) = probe_over_column(table, source, probe_index, flavor)?;
    let batch_rows = source.batch_rows_under(MAX_READER_BATCH_ROWS);
    let value_index = source.column_index("l_extendedprice").unwrap_or(probe_index);
    let (batches, batched_rows) =
        probe_in_batches(path, &[probe_index, value_index], batch_rows, table)?;

    let values = probe_values(source, probe_index, flavor)?;
    let counts = candidate_counts(&build.collected, build.base, build.width);
    let all_rows = 0..u32::try_from(source.rows()).unwrap_or(u32::MAX);
    let candidate_rows = candidate_probe(&counts, build.base, &values, all_rows.clone());
    assert_eq!(candidate_rows, engine_rows, "the candidate probes differently");
    assert_eq!(candidate_rows, batched_rows, "the two paths probe differently");

    let floor = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = floor_probe(&counts, build.base, &values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });
    let candidate = repeat(ITERATIONS, || {
        let started = Instant::now();
        let rows = candidate_probe(&counts, build.base, &values, all_rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(&rows);
        elapsed
    });
    Ok(ProbeParts { column, batches, floor, candidate, engine_rows, values })
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
fn measure(path: &Path) -> Result<Measured, Box<dyn std::error::Error>> {
    let flavor = columnar_kernels::best();
    let options = ExecOptions::default();
    let source = ParquetSource::open(path)?;
    let rows = source.rows();

    let plan = (definition().plan)(&build_path_for(path));
    let dataset = Dataset::read_parquet(path)?;
    let (stage_cold, stage_resident, cold_rows, resident_rows) =
        engine_stages(&plan, &source, &dataset, &options);

    let spec = plan.join.as_ref().ok_or("the plan has a join")?;
    let build = build_parts(spec, flavor)?;
    let table = build_table(&ParquetSource::open(&spec.build_path)?, spec, flavor, 1)?;
    let probe = probe_parts(path, &source, &table, &build, flavor)?;

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

    let parts = vec![
        ("stage_cold", stage_cold),
        ("stage_resident", stage_resident),
        ("build_open", build.open),
        ("build_read", build.read),
        ("build_keys", build.keys),
        ("build_whole", build.whole),
        ("table_own", build.table),
        ("probe_column", probe.column),
        ("probe_batches", probe.batches),
        ("floor_probe", probe.floor),
        ("candidate_probe", probe.candidate),
        ("read_plain", read_plain_timing),
        ("read_small", read_small),
    ];
    Ok(Measured {
        parts,
        cold_rows,
        resident_rows,
        engine_rows: probe.engine_rows.len(),
        table_rows: table.rows(),
        rows,
    })
}

/// The report, and the printout that goes with it.
fn report(out: &Path, path: &Path, measured: &Measured) -> Result<(), Box<dyn std::error::Error>> {
    let mut parts_ms = serde_json::Map::new();
    let mut spreads_ms = serde_json::Map::new();
    for (name, timing) in &measured.parts {
        parts_ms.insert((*name).to_owned(), serde_json::json!(timing.median));
        spreads_ms.insert((*name).to_owned(), serde_json::json!(timing.spread()));
    }
    let part = |name: &str| -> f64 {
        measured
            .parts
            .iter()
            .find(|(part, _)| *part == name)
            .map_or(f64::NAN, |(_, timing)| timing.median)
    };
    let report = serde_json::json!({
        "probe": "join-cost",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "commit": commit(),
        "host_load_average": load_average(),
        "rows": measured.rows,
        "probe_key": PROBE_KEY,
        "build_key": BUILD_KEY,
        "parts_ms": parts_ms,
        "spreads_ms": spreads_ms,
        "derived_ms": {
            "build_table": part("build_whole") - part("build_read") - part("build_keys"),
            "output_rows": part("candidate_probe") - part("floor_probe"),
            "the_probe_s_remainder": part("probe_column") - part("candidate_probe"),
            "the_two_paths": part("stage_cold") - part("stage_resident"),
            "the_two_probes": part("probe_batches") - part("probe_column"),
        },
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
    let (path, out) = arguments()?;
    let measured = measure(&path)?;
    report(&out, &path, &measured)
}
