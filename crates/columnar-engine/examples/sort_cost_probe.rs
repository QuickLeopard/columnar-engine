//! The sort's stage, taken apart.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind spec 025's question: **what is in the sort's stage that is not the
//! sort's own work?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example sort_cost_probe -- \
//!     data/lineitem.parquet --out report.json
//! ```
//!
//! # The question
//!
//! The plan output is the engine's own account of its work, and it says the
//! sort's stage is **4.9 ms** of the cold `sort-topk` and the whole of the
//! resident one. The published figure for the sort's **operator** is **3.7 ms**
//! against a **3.6 ms** plain-read floor -- but that was measured two
//! increments ago, on a different code version, with the rows handed over as a
//! vector, and the floor it measured was itself a scalar read. The stage, its
//! operator and the floor have never been measured against each other.
//!
//! # The parts
//!
//! | | |
//! |---|---|
//! | **`stage_cold`**, **`stage_resident`** | the engine's own sort stage, as the plan output reports it, on both paths |
//! | **`operator_cold`**, **`operator_warm`** | the sort's own call, over the same key column and the same rows |
//! | **`read_shape`** | the read the engine's fast path does -- a row at a time, with a bounds check -- without the heap |
//! | **`read_plain`** | the plain read of the key column: the published floor |
//! | **`read_small`** | the same read over a **small** buffer: the control that says the read is not memory-bound |
//! | **`floor_block`** | the **vectorised** comparison of the same column: the floor a block comparison reaches |
//! | **`candidate_block`** | the sort written here, over the same column, in blocks: the cross-check |
//! | **`rows_range`** | the hand-over: the range of rows the sort is given, walked |
//! | **`key_column`** | the key column's construction, from the dataset's chunk |
//!
//! # What the floor is
//!
//! A top-k over six million keys has to look at every one of them, so the
//! **read** of the 48 MB column is a floor no algorithm gets under. But the
//! read is not the whole cost: the **comparison** is one per key, and a
//! comparison made a row at a time is a scalar loop. The probe measures both
//! -- the plain read, and the comparison in blocks -- because the difference
//! between them is what the sort's own work costs.
//!
//! The **control** is `read_small`: the same read over a buffer small enough
//! to be in the caches. A read that costs the same over four megabytes as it
//! does over forty-eight is not waiting for memory; it is counting.
//!
//! # Cold and hot
//!
//! The operator is measured twice: **cold**, with the caches pushed out
//! first, and **hot**, with the column already in them. The difference is
//! the column's **cache state**.
//!
//! # The candidate
//!
//! **`fused_decode`** and **`fused_topk`** are the **scan that feeds the
//! sort**: the top-k is computed over the reader's own Arrow arrays, where
//! the values are hot, instead of over the materialised column. The decode is
//! measured on its own and with the top-k on top of it, so the top-k's own
//! cost is the difference between the two.
//!
//! # The answer is checked first
//!
//! The probe runs the engine's **own** plan through the engine's own entry
//! point and checks the answer against the published one before anything is
//! timed. Every candidate is checked against the engine's rows, **row for
//! row and in order**, before it is timed: a candidate that sorts differently
//! is not a candidate, it is a different sort.
//!
//! # The fallbacks are named
//!
//! **`operator_all`** is the sort with **no limit**: it sorts every row, and
//! its cost is not the bounded sort's cost. **`operator_text`** is the sort
//! over a **string** column: the general path reads a key as an enum, and
//! the published query's path does not.
//!
//! # What the figures are not
//!
//! These are figures on a **shared** host, taken five times each and reported
//! as the median with the spread beside it. The report carries the host's load
//! average, so a figure that drifted can be read against it.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arrow::array::{Array, Float64Array};
use columnar_engine::operators::key::KeyColumn;
use columnar_engine::operators::sort;
use columnar_engine::queries::BuiltinQuery;
use columnar_engine::{Dataset, ExecOptions, ParquetSource, PushdownPlan, MAX_READER_BATCH_ROWS};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;

/// The key column the published sort reads.
const KEY: &str = "l_extendedprice";

/// The column the general path is measured over.
const TEXT_KEY: &str = "l_shipmode";

/// The limit the published sort takes.
const LIMIT: usize = 10;

/// How many times each part is measured.
const ITERATIONS: usize = 5;

/// How many times the fallbacks are measured.
const FALLBACK_ITERATIONS: usize = 3;

/// How much memory the cache flush touches.
const FLUSH_BYTES: usize = 256 << 20;

/// How many keys the small control holds.
///
/// It is small enough to be in the caches whatever happens, so a read that
/// costs the same over it as over the whole column is not waiting for memory.
const SMALL_KEYS: usize = 512 * 1024;

/// The answer the published sort gives.
///
/// The sum of the ten largest `l_extendedprice` values, over six million rows.
const EXPECTED: f64 = 1_049_999.6;

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
        .and_then(|text| {
            text.replace(['{', '}'], " ")
                .split_whitespace()
                .next()
                .and_then(|first| first.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
}

/// The definition of the published sort.
fn definition() -> &'static columnar_engine::queries::QueryDefinition {
    columnar_engine::queries::QUERIES
        .iter()
        .find(|definition| definition.query == BuiltinQuery::SortTopK)
        .expect("the sort is a builtin one")
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

/// The sort's stage, as the plan output reports it.
///
/// The stage is the engine's own account, so it is read from the plan output
/// rather than measured beside it.
fn stage_of(result: &columnar_engine::AggregateResult) -> Duration {
    result
        .stages
        .iter()
        .find(|stage| stage.stage == "sort")
        .map_or(Duration::ZERO, |stage| stage.elapsed)
}

/// The key column's values, as a plain slice of doubles.
///
/// The column the published sort reads holds plain doubles and no NULLs, so
/// the plain read is a read of the same buffer the sort reads.
fn values_of(chunk: &columnar_core::ColumnChunk) -> &[f64] {
    match &chunk.encoding {
        columnar_core::Encoding::Float64 { values } => values,
        other => panic!("the sort's key is not a plain float: {other:?}"),
    }
}

/// A buffer that is touched to push the caches out.
///
/// The column the sort reads is 48 MB, and the machine's caches are smaller
/// than that, so a column that has just been read is still in them. A "cold"
/// measurement has to push them out first, or it measures a hot read and
/// calls it cold.
struct Flush {
    /// The buffer the flush touches.
    bytes: Vec<u8>,
}

impl Flush {
    /// A flush buffer of [`FLUSH_BYTES`].
    fn new() -> Self {
        Self { bytes: vec![0u8; FLUSH_BYTES] }
    }

    /// Touches every cache line of the buffer.
    fn touch(&mut self) {
        let mut total = 0u64;
        for line in self.bytes.chunks_exact(64) {
            total += u64::from(line.first().copied().unwrap_or(0));
        }
        std::hint::black_box(total);
    }
}

/// The plain read of a column: the floor as it was published.
///
/// It is a sum, and the compiler turns it into a loop that counts the values
/// one at a time -- which is the point: it is the fastest way to read the
/// column **a row at a time**, and the sort cannot be faster than that.
fn read_plain(values: &[f64]) -> f64 {
    let mut total = 0.0_f64;
    for value in values {
        total += *value;
    }
    std::hint::black_box(&total);
    total
}

/// The read the engine's fast path does, without the heap.
///
/// The engine reads a key a row at a time, out of the column's own slice,
/// with a bounds check and a `usize` conversion per row. This is that read,
/// and nothing else: the heap's work is the difference between this and the
/// operator.
fn read_shape(values: &[f64], rows: std::ops::Range<u32>) -> f64 {
    let mut total = 0.0_f64;
    for row in rows {
        let value =
            values.get(usize::try_from(row).unwrap_or(usize::MAX)).copied().unwrap_or_default();
        total += value;
    }
    std::hint::black_box(&total);
    total
}

/// The walk of a range of rows: the hand-over's own cost.
///
/// The mask is opaque to the compiler: a plain sum of `0..n` is a closed form
/// the compiler computes without walking anything.
fn walk_range(rows: usize, mask: u32) -> u64 {
    let mut total = 0u64;
    for row in 0..rows {
        total += u64::from(u32::try_from(row).unwrap_or(0) ^ mask);
    }
    std::hint::black_box(&total);
    total
}

/// The value's key in the total order.
///
/// It is the transform `total_cmp` uses: a negative double's bits are
/// flipped, so that the integers' order is the doubles' order.
#[inline]
fn total_key(value: f64) -> i64 {
    let bits = i64::from_ne_bytes(value.to_bits().to_ne_bytes());
    bits ^ ((bits >> 63) & i64::MAX)
}

/// Counts the values that are at least as good as the threshold.
///
/// It is the whole of the comparison's work, and it is a reduction over a
/// comparison -- the shape a compiler vectorises.
#[inline]
fn count_at_least(values: &[f64], threshold: i64) -> u64 {
    let mut count = 0u64;
    for value in values {
        count += u64::from(total_key(*value) >= threshold);
    }
    count
}

/// One row in the probe's own heap.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// The row's key.
    key: f64,
    /// The row's own number.
    row: u32,
}

/// The engine's order: the key's order, the direction, and the row's number.
///
/// It is the engine's own rule, written out here so that the probe's
/// candidate is the engine's sort and not a sort that looks like it.
fn order(left: Entry, right: Entry) -> Ordering {
    let ordering = left.key.total_cmp(&right.key);
    let ordering = ordering.reverse();
    if ordering == Ordering::Equal {
        left.row.cmp(&right.row)
    } else {
        ordering
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    /// The greatest entry is the one that sorts **last**: the row that would
    /// be dropped first.
    fn cmp(&self, other: &Self) -> Ordering {
        order(*self, *other)
    }
}

/// The bounded top-k over a contiguous run, in blocks.
///
/// It is the engine's own sort, written out here: the candidate the engine's
/// sort is measured against, and the cross-check that the engine's answer is
/// the answer this one gives.
fn candidate_topk(values: &[f64], limit: usize, block: usize) -> Vec<u32> {
    let mut heap: BinaryHeap<Entry> = BinaryHeap::with_capacity(limit + 1);
    let mut threshold = i64::MIN;
    let mut row = 0u32;
    let mut rest = values;
    while !rest.is_empty() {
        let take = block.min(rest.len());
        let (chunk, tail) = rest.split_at(take);
        rest = tail;
        if count_at_least(chunk, threshold) == 0 {
            row = row.saturating_add(u32::try_from(take).unwrap_or(u32::MAX));
            continue;
        }
        for (offset, value) in chunk.iter().enumerate() {
            let candidate = row.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            let entry = Entry { key: *value, row: candidate };
            if heap.len() >= limit {
                let better =
                    heap.peek().is_some_and(|worst| order(entry, *worst) == Ordering::Less);
                if !better {
                    continue;
                }
                heap.pop();
            }
            heap.push(entry);
            if heap.len() >= limit {
                if let Some(worst) = heap.peek() {
                    threshold = total_key(worst.key);
                }
            }
        }
        row = row.saturating_add(u32::try_from(take).unwrap_or(u32::MAX));
    }
    let mut entries = heap.into_vec();
    entries.sort_unstable_by(|left, right| order(*left, *right));
    entries.into_iter().map(|entry| entry.row).collect()
}

/// The reader's batches, and the top-k over them.
///
/// This is the **scan that feeds the sort**: the top-k is computed over the
/// reader's own Arrow arrays, where the values are hot, rather than over the
/// materialised column.
///
/// With `topk` false the batches are only decoded, which is the control: the
/// top-k's own cost is the difference between the two.
fn scan_topk(
    path: &Path,
    column: usize,
    batch_rows: usize,
    limit: usize,
    topk: bool,
) -> Result<(Vec<u32>, Duration), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let mask = ProjectionMask::roots(builder.parquet_schema(), [column]);
    let reader = builder.with_projection(mask).with_batch_size(batch_rows).build()?;

    let started = Instant::now();
    let mut heap: BinaryHeap<Entry> = BinaryHeap::with_capacity(limit + 1);
    let mut base = 0u32;
    for batch in reader {
        let batch = batch?;
        let values = batch
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or("the key column is not a float column")?;
        if topk {
            for (offset, value) in values.iter().enumerate() {
                let Some(value) = value else { continue };
                let row = base.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
                keep(&mut heap, Entry { key: value, row }, limit);
            }
        }
        base = base.saturating_add(u32::try_from(values.len()).unwrap_or(u32::MAX));
    }
    let elapsed = started.elapsed();

    let mut entries = heap.into_vec();
    entries.sort_unstable_by(|left, right| order(*left, *right));
    Ok((entries.into_iter().map(|entry| entry.row).collect(), elapsed))
}

/// Keeps the best `limit` rows in the heap.
///
/// It is the engine's own rule: the heap's top is the worst row, and a row
/// that is not better than the top is dropped on the spot.
fn keep(heap: &mut BinaryHeap<Entry>, entry: Entry, limit: usize) {
    if heap.len() >= limit {
        let better = heap.peek().is_some_and(|worst| order(entry, *worst) == Ordering::Less);
        if !better {
            return;
        }
        heap.pop();
    }
    heap.push(entry);
}

/// Checks a query's answer against the published one.
fn check(result: &columnar_engine::AggregateResult) {
    let value = value_of(result);
    let tolerance = EXPECTED.abs().max(1.0) * 1e-6;
    assert!(
        (value - EXPECTED).abs() <= tolerance,
        "the sort's answer is {value}, the published answer is {EXPECTED}"
    );
}

/// The column the parts are measured over.
struct Column<'a> {
    /// The key column's values.
    values: &'a [f64],
    /// The key column, as the operators read it.
    keys: KeyColumn<'a>,
    /// The rows the sort is handed: every row of the file.
    rows: std::ops::Range<u32>,
}

impl Column<'_> {
    /// The engine's own sort, over the column and the rows.
    fn sort(&self, limit: Option<usize>) -> Vec<u32> {
        sort::sort_rows(self.rows.clone(), &self.keys, true, limit)
    }
}

/// The engine's own stage, on both paths.
///
/// The plan is the engine's own, and the answer is checked against the
/// published one before anything is timed.
fn engine_stages(
    plan: &columnar_engine::QueryPlan,
    source: &ParquetSource,
    dataset: &Dataset,
    options: &ExecOptions,
) -> (Timing, Timing) {
    let mut cold = Vec::with_capacity(ITERATIONS);
    let mut resident = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let over_file = columnar_engine::execute_on(plan, source, options).expect("the query runs");
        check(&over_file);
        cold.push(stage_of(&over_file));

        let over_memory = columnar_engine::execute(plan, dataset, options).expect("the query runs");
        check(&over_memory);
        resident.push(stage_of(&over_memory));
    }
    (Timing::of(&cold), Timing::of(&resident))
}

/// The operator, cold and hot.
fn operator_parts(column: &Column<'_>, flush: &mut Flush) -> [Timing; 2] {
    let cold = repeat(ITERATIONS, || {
        flush.touch();
        let started = Instant::now();
        let sorted = column.sort(Some(LIMIT));
        let elapsed = started.elapsed();
        std::hint::black_box(&sorted);
        elapsed
    });
    let warm = repeat(ITERATIONS, || {
        let started = Instant::now();
        let sorted = column.sort(Some(LIMIT));
        let elapsed = started.elapsed();
        std::hint::black_box(&sorted);
        elapsed
    });
    [cold, warm]
}

/// The reads: the engine's shape, the plain read, and the small control.
fn read_parts(column: &Column<'_>, small: &[f64]) -> [Timing; 3] {
    let shape = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = read_shape(column.values, column.rows.clone());
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });
    let plain = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = read_plain(column.values);
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });
    // The control: the same read over a buffer that is in the caches.
    let control = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = read_plain(small);
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });
    [shape, plain, control]
}

/// The vectorised comparison: the floor a block comparison reaches.
///
/// The comparison is made in blocks, the way the engine's sort makes it, and
/// the threshold is opaque to the compiler: a threshold of the lowest value
/// would fold the count to the column's length.
fn floor_parts(column: &Column<'_>) -> Timing {
    let threshold = total_key(std::hint::black_box(0.5_f64));
    repeat(ITERATIONS, || {
        let started = Instant::now();
        let mut count = 0u64;
        for block in column.values.chunks(1024) {
            count += count_at_least(block, threshold);
        }
        let elapsed = started.elapsed();
        std::hint::black_box(count);
        elapsed
    })
}

/// The candidate: the sort written here, in blocks.
///
/// It is checked against the engine's rows, row for row and in order,
/// before it is timed.
fn candidate_parts(column: &Column<'_>, engine_rows: &[u32]) -> Timing {
    let rows = candidate_topk(column.values, LIMIT, 1024);
    assert_eq!(rows, engine_rows, "the candidate sorts differently");
    repeat(ITERATIONS, || {
        let started = Instant::now();
        let sorted = candidate_topk(column.values, LIMIT, 1024);
        let elapsed = started.elapsed();
        std::hint::black_box(&sorted);
        elapsed
    })
}

/// The hand-over's own cost, and the key column's construction.
fn hand_over_parts(rows: usize, chunk: &columnar_core::ColumnChunk) -> [Timing; 2] {
    let mask = std::hint::black_box(0x9E37_79B9_u32);
    let walk = repeat(ITERATIONS, || {
        let started = Instant::now();
        let total = walk_range(rows, mask);
        let elapsed = started.elapsed();
        std::hint::black_box(total);
        elapsed
    });
    // The construction is nanoseconds, so a thousand of them are measured and
    // the median is divided: one construction is below the clock's resolution.
    let construction = repeat(ITERATIONS, || {
        let started = Instant::now();
        for _ in 0..1_000 {
            let keys = KeyColumn::from_chunk(chunk);
            std::hint::black_box(&keys);
        }
        let elapsed = started.elapsed();
        elapsed / 1_000
    });
    [walk, construction]
}

/// The alternative: the scan that feeds the sort.
fn fused_parts(
    path: &Path,
    column: usize,
    batch_rows: usize,
    engine_rows: &[u32],
) -> Result<(Timing, Timing), Box<dyn std::error::Error>> {
    let (rows, _) = scan_topk(path, column, batch_rows, LIMIT, true)?;
    assert_eq!(rows, engine_rows, "the fused candidate sorts differently");
    let decode = repeat(ITERATIONS, || {
        let (_, elapsed) =
            scan_topk(path, column, batch_rows, LIMIT, false).expect("the file reads");
        elapsed
    });
    let topk = repeat(ITERATIONS, || {
        let (_, elapsed) =
            scan_topk(path, column, batch_rows, LIMIT, true).expect("the file reads");
        elapsed
    });
    Ok((decode, topk))
}

/// The sort over a string column: the general path.
///
/// The general path reads a key as an enum, because the column's values are
/// not numbers, and the published query's path does not.
fn text_sort(
    source: &ParquetSource,
    pushdown: &PushdownPlan,
    flavor: columnar_kernels::Flavor,
) -> f64 {
    let Some(index) = source.column_index(TEXT_KEY) else {
        return f64::NAN;
    };
    let materialised = source.read(&[index], pushdown, flavor, 1).expect("the file reads");
    let Some(chunk) = materialised.chunk(TEXT_KEY) else {
        return f64::NAN;
    };
    let keys = KeyColumn::from_chunk(chunk);
    let rows = u32::try_from(materialised.len()).unwrap_or(u32::MAX);
    let started = Instant::now();
    let sorted = sort::sort_rows(0..rows, &keys, true, Some(LIMIT));
    let elapsed = started.elapsed();
    std::hint::black_box(&sorted);
    ms(elapsed)
}

/// The sort with no limit: every row is sorted.
fn unbounded_sort(column: &Column<'_>) -> Timing {
    repeat(FALLBACK_ITERATIONS, || {
        let started = Instant::now();
        let sorted = column.sort(None);
        let elapsed = started.elapsed();
        std::hint::black_box(&sorted);
        elapsed
    })
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
                println!("usage: sort_cost_probe --data <parquet> [--out <report.json>]");
                std::process::exit(0);
            }
            // The path may also be given on its own, the way the other probes
            // take it.
            other if !other.starts_with("--") => path = Some(PathBuf::from(other)),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let path = path.ok_or("--data is required")?;
    let out = out.unwrap_or_else(|| PathBuf::from("sort-cost.json"));
    Ok((path, out))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (path, out) = arguments()?;
    let flavor = columnar_kernels::best();
    let options = ExecOptions::default();
    let source = ParquetSource::open(&path)?;
    let rows = source.rows();
    let key_index = source.column_index(KEY).ok_or_else(|| format!("no {KEY} column"))?;
    let pushdown = PushdownPlan::default();

    let plan = (definition().plan)(path.parent().unwrap_or(Path::new(".")));
    let dataset = Dataset::read_parquet(&path)?;
    let (stage_cold, stage_resident) = engine_stages(&plan, &source, &dataset, &options);

    let materialised = source.read(&[key_index], &pushdown, flavor, 1)?;
    let chunk = materialised.chunk(KEY).ok_or("the materialised column is there")?;
    let column = Column {
        values: values_of(chunk),
        keys: KeyColumn::from_chunk(chunk),
        rows: 0..u32::try_from(rows).unwrap_or(u32::MAX),
    };
    let engine_rows = column.sort(Some(LIMIT));
    let small: Vec<f64> =
        (0..SMALL_KEYS).map(|index| f64::from(u8::try_from(index % 97).unwrap_or(0))).collect();

    let mut flush = Flush::new();
    let [operator_cold, operator_warm] = operator_parts(&column, &mut flush);
    let [read_shape, read_plain, read_small] = read_parts(&column, &small);
    let floor_block = floor_parts(&column);
    let candidate_block = candidate_parts(&column, &engine_rows);
    let [rows_range, key_column] = hand_over_parts(rows, chunk);

    let batch_rows = source.batch_rows_under(MAX_READER_BATCH_ROWS);
    let (fused_decode, fused_topk) = fused_parts(&path, key_index, batch_rows, &engine_rows)?;

    let operator_all = unbounded_sort(&column);
    let operator_text = text_sort(&source, &pushdown, flavor);

    let parts = [
        ("stage_cold", stage_cold),
        ("stage_resident", stage_resident),
        ("operator_cold", operator_cold),
        ("operator_warm", operator_warm),
        ("read_shape", read_shape),
        ("read_plain", read_plain),
        ("read_small", read_small),
        ("floor_block", floor_block),
        ("candidate_block", candidate_block),
        ("rows_range", rows_range),
        ("key_column", key_column),
        ("fused_decode", fused_decode),
        ("fused_topk", fused_topk),
    ];
    let mut parts_ms = serde_json::Map::new();
    let mut spreads_ms = serde_json::Map::new();
    for (name, timing) in parts {
        parts_ms.insert(name.to_owned(), serde_json::json!(timing.median));
        spreads_ms.insert(name.to_owned(), serde_json::json!(timing.spread()));
    }
    let report = serde_json::json!({
        "probe": "sort-cost",
        "file": path.display().to_string(),
        "iterations": ITERATIONS,
        "fallback_iterations": FALLBACK_ITERATIONS,
        "commit": commit(),
        "host_load_average": load_average(),
        "rows": rows,
        "key": KEY,
        "limit": LIMIT,
        "parts_ms": parts_ms,
        "spreads_ms": spreads_ms,
        "fallbacks_ms": {
            "operator_all": operator_all.median,
            "operator_text": operator_text,
        },
        "engine_rows": engine_rows,
    });
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(&out, format!("{json}\n"))?;

    println!("\n{:<22}{:>12}", "the part", "ms");
    for (name, timing) in parts {
        println!("{name:<22}{:>12.3}", timing.median);
    }
    println!("{:<22}{:>12.3}", "operator_all", operator_all.median);
    println!("{:<22}{:>12.3}", "operator_text", operator_text);
    println!("\nwrote {}", out.display());
    Ok(())
}
