//! The build side's keys as a filter: the ladder, and the filter's verdict.
//!
//! Spec 046 took the `join-orders` query apart on both sides and found the
//! gap: `DuckDB` joins **216,783** rows and this engine joins **6,000,000**,
//! because `DuckDB` pushes the build side's keys into the probe side's scan as
//! a filter and this engine has no such filter. **And `DuckDB`'s own join, over
//! the same six million rows, costs far more than this engine's** -- so the
//! engine is not behind because it looks up slowly.
//!
//! **This probe asks whether a filter can close the difference.** It is the
//! house's ladder, and every rung adds exactly one thing to the rung below it:
//!
//! | rung | what it is |
//! |---|---|
//! | **`floor_table`** | the table's own load, one a probe row: the byte, out of the table's own memory |
//! | **`floor_filter`** | the filter's own load, one a probe row: the bit, out of the filter's own memory |
//! | **`candidate_table`** | the probe as it is: the table's load, and the output rows |
//! | **`candidate_filter`** | the probe with the filter in front of it: the filter's load, and the table's load for the rows the filter lets through |
//! | **`engine_probe`** | the engine's own probe, over the same column -- the rung that says the candidates are the engine's probe |
//!
//! **And the cross-check**: the two candidates' rows are compared with each
//! other, and with the engine's own probe's rows, **row for row, in order**.
//! A filter may let a key through that is not in the build side -- that is
//! allowed, and the table decides -- but it may **never** refuse a key that is
//! in it, and the comparison is what says so.
//!
//! # The column is materialised afresh, every iteration
//!
//! The engine's own query materialises the probe column and then probes it, so
//! the column it probes is one the reader has **just written**. A rung that
//! read a buffer kept warm across the iterations would be measured on a
//! **cheaper** column than the engine pays for, and the difference between the
//! two would be the **buffer's**, not the lookup's.
//!
//! **That is the whole of the filter's case.** The table is **six million
//! bytes** and the filter is **three quarters of a megabyte**; the filter only
//! wins if the probe column's stream evicts the table and leaves the filter.
//! A column held warm across the iterations would not evict anything, and the
//! filter would be measured on a shape that does not exist.
//!
//! # The order alternates
//!
//! Every rung is measured **in the iteration**, and the two sides of every
//! difference are measured **back to back with the order alternated**: a
//! machine that drifts while an iteration runs gives the first measurement a
//! different price from the second. **The iteration count is even**, for the
//! reason spec 026 gave.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use columnar_core::{ColumnChunk, ComparisonOp, ScalarValue};
use columnar_engine::operators::join::{build_table, probe_rows, BuildTable};
use columnar_engine::operators::key::KeyColumn;
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::queries::{BuiltinQuery, QUERIES};
use columnar_engine::ParquetSource;
use columnar_kernels::Flavor;

/// The probe side's key column.
const PROBE_KEY: &str = "l_orderkey";

/// The build side's key column.
const BUILD_KEY: &str = "o_orderkey";

/// The build side's filter column.
const BUILD_DATE: &str = "o_orderdate";

/// The rows the published join produces.
const EXPECTED_ROWS: usize = 216_783;

/// How many times each rung is measured.
///
/// **Even**, so that the two orders' groups are the same size.
const ITERATIONS: usize = 6;

/// The candidates, in the order one iteration runs them.
///
/// `0` is the probe as it is, `1` the probe with the filter compacting the
/// rows in front of it, and `2` the probe with the filter in front of the
/// table.
///
/// **The six orders are chosen so that every pair is adjacent in four of the
/// six iterations, two in each direction.** A difference is only taken between
/// two measurements that were **back to back**, and the order within the pair
/// is **alternated**, so that the machine's drift cannot land on one side --
/// the reason spec 026 gave. A rotation of three would have put one pair
/// adjacent four times and another twice; these six put every pair on the
/// same footing, and every candidate takes every position exactly twice.
const CANDIDATE_NAMES: [&str; 3] = ["table", "compacted", "filter"];

/// The six orders, as the candidates' names.
const ORDERS: [[usize; 3]; ITERATIONS] =
    [[0, 1, 2], [1, 0, 2], [2, 0, 1], [1, 2, 0], [2, 1, 0], [0, 2, 1]];

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

/// The host's load average.
///
/// **macOS first**: `sysctl` reports `{ 2.31 3.02 3.44 }` and Linux's
/// `/proc/loadavg` reports `2.31 3.02 3.44 1/2345 6789`; both start with the
/// one-minute average, which is the number a reader compares.
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

/// The build side's own range: the base its table addresses from, and the
/// width it addresses.
fn build_range(source: &ParquetSource) -> (i64, usize) {
    source.column_range(BUILD_KEY).map_or((0_i64, 0_usize), |range| {
        let (ScalarValue::Int64(min), ScalarValue::Int64(max)) = range else {
            return (0_i64, 0_usize);
        };
        (min, usize::try_from(max.saturating_sub(min).saturating_add(1)).unwrap_or(0))
    })
}

/// The build side's keys, collected from its column.
///
/// It is the engine's own walk -- a row at a time, out of the column's own
/// values -- and the table is built from what it collects.
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

/// The instrument's own table: one byte a count, over the key's own range.
///
/// It is the engine's own table's shape, and it is built from the same keys.
fn counts(keys: &[i64], base: i64, width: usize) -> Vec<u8> {
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

/// The instrument's own filter: one bit a key, over the same range.
///
/// It is the engine's own filter's shape.
fn bits(keys: &[i64], base: i64, width: usize) -> Vec<u64> {
    let mut bits = vec![0_u64; width.div_ceil(64)];
    for key in keys {
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        if offset >= width {
            continue;
        }
        if let Some(word) = bits.get_mut(offset / 64) {
            *word |= 1_u64 << (offset % 64);
        }
    }
    bits
}

/// The table's own load, one a probe row.
///
/// It is the lookup without anything else -- no output rows, no `Option`, no
/// enum -- and it is what a probe over a direct-addressed table cannot be
/// faster than.
fn floor_table(counts: &[u8], base: i64, values: &[i64]) -> u64 {
    let mut total = 0_u64;
    for key in values {
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        total += u64::from(counts.get(offset).copied().unwrap_or(0));
    }
    std::hint::black_box(&total);
    total
}

/// The filter's own load, one a probe row.
fn floor_filter(bits: &[u64], base: i64, values: &[i64]) -> u64 {
    let mut total = 0_u64;
    for key in values {
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        let word = bits.get(offset / 64).copied().unwrap_or(0);
        total += word >> (offset % 64) & 1;
    }
    std::hint::black_box(&total);
    total
}

/// The instrument's own table: the engine's own table's shape.
///
/// It is the direct-addressed table the engine builds -- one byte a count, over
/// the key's own range, and the overflow map the engine consults only when it
/// holds something -- **without** the filter in front of it, so that the
/// probe as it is can be measured.
#[derive(Debug)]
struct Table {
    /// The count a key accounts for, one byte a key.
    counts: Vec<u8>,
    /// The first key the table addresses from.
    base: i64,
    /// The keys more than 255 build rows carry.
    ///
    /// **`None` when no key overflowed**, which is the published file's shape:
    /// the build side's keys are distinct, so no byte fills.
    overflow: Option<std::collections::HashMap<i64, u32>>,
}

impl Table {
    /// How many build rows carry `key`.
    ///
    /// **It is the engine's own lookup**, line for line.
    #[inline]
    fn count(&self, key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        let byte = self.counts.get(index).copied().map_or(0, u32::from);
        let Some(overflow) = &self.overflow else {
            return byte;
        };
        let extra = overflow.get(&key).copied().unwrap_or(0);
        byte.saturating_add(extra)
    }

    /// How many build rows carry `key`, with the filter in front of the table.
    ///
    /// **It is the engine's own lookup with the filter in it**, line for line:
    /// the key's place in the range is computed once, the filter is handed it,
    /// and a key the filter refuses does not read the table's byte.
    #[inline]
    fn filtered_count(&self, bits: &[u64], key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        if index >= bits.len() * 64 {
            return 0;
        }
        let word = bits.get(index / 64).copied().unwrap_or(0);
        if word >> (index % 64) & 1 == 0 {
            return 0;
        }
        let byte = self.counts.get(index).copied().map_or(0, u32::from);
        let Some(overflow) = &self.overflow else {
            return byte;
        };
        let extra = overflow.get(&key).copied().unwrap_or(0);
        byte.saturating_add(extra)
    }
}

/// The probe as it is: the engine's own probe, without the filter.
///
/// **It is the engine's `probe_direct`, line for line**: the fast path over
/// the key column's own integers, the row's number turned into an index, the
/// key read out of the column, the count, and the row pushed once a build row
/// carries the key.
fn candidate_table<I>(table: &Table, keys: &KeyColumn<'_>, rows: I) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    let mut out: Vec<u32> = Vec::new();
    let Some(values) = keys.integer_values() else {
        return out;
    };
    if keys.has_nulls() {
        return out;
    }
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

/// The probe with the filter in front of the table.
///
/// **It is the engine's `probe_direct`, line for line, with the filter in the
/// lookup**: the filter's test is the first thing a row meets, and a row it
/// refuses does not read the table's byte at all.
fn candidate_filter<I>(table: &Table, bits: &[u64], keys: &KeyColumn<'_>, rows: I) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    let mut out: Vec<u32> = Vec::new();
    let Some(values) = keys.integer_values() else {
        return out;
    };
    if keys.has_nulls() {
        return out;
    }
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let count = table.filtered_count(bits, key);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The probe with the filter **compacting** the rows in front of it.
///
/// This is the other shape: the filter's test runs over every row, and the
/// **survivors alone** are probed. It is the shape in which the filter can
/// keep the rows out of the probe's own work -- and it is the shape the
/// prize is about, because the probe over the survivors is the join the
/// engine would be left with.
fn candidate_compacted<I>(table: &Table, bits: &[u64], keys: &KeyColumn<'_>, rows: I) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    let mut out: Vec<u32> = Vec::new();
    let Some(values) = keys.integer_values() else {
        return out;
    };
    if keys.has_nulls() {
        return out;
    }
    // The compaction: the filter's test, over every row.
    let mut survivors: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(&key) = values.get(index) else {
            continue;
        };
        let offset = usize::try_from(key.saturating_sub(table.base)).unwrap_or(usize::MAX);
        if offset >= bits.len() * 64 {
            continue;
        }
        let word = bits.get(offset / 64).copied().unwrap_or(0);
        if word >> (offset % 64) & 1 == 0 {
            continue;
        }
        survivors.push(row);
    }
    // And the probe, over the survivors alone.
    for row in survivors {
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

/// The table's own construction: the bytes, and the counts over them.
///
/// It is the engine's own table's construction -- the table is built before
/// the scan, and the join's stage pays for it.
fn construct_table(keys: &[i64], base: i64, width: usize) -> Vec<u8> {
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

/// The filter's own construction: the bits, and the keys over them.
fn construct_filter(keys: &[i64], base: i64, width: usize) -> Vec<u64> {
    let mut bits = vec![0_u64; width.div_ceil(64)];
    for key in keys {
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        if offset >= width {
            continue;
        }
        if let Some(word) = bits.get_mut(offset / 64) {
            *word |= 1_u64 << (offset % 64);
        }
    }
    bits
}

/// The rows the filter lets through.
fn survivors(bits: &[u64], base: i64, values: &[i64]) -> Vec<u32> {
    let mut rows: Vec<u32> = Vec::new();
    for (row, key) in values.iter().enumerate() {
        let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
        if offset >= bits.len() * 64 {
            continue;
        }
        let word = bits.get(offset / 64).copied().unwrap_or(0);
        if word >> (offset % 64) & 1 == 1 {
            rows.push(u32::try_from(row).unwrap_or(u32::MAX));
        }
    }
    rows
}

/// Were the two candidates back to back in this order?
///
/// **A difference is only taken between two measurements that were measured
/// back to back**: the machine's drift between them would land on the
/// difference, and the difference is the figure.
fn adjacent(order: &str, left: &str, right: &str) -> bool {
    let parts: Vec<&str> = order.split(',').collect();
    let left = parts.iter().position(|part| *part == left);
    let right = parts.iter().position(|part| *part == right);
    match (left, right) {
        (Some(left), Some(right)) => left.abs_diff(right) == 1,
        _ => false,
    }
}

/// One iteration's measurements.
#[derive(Debug, Clone, serde::Serialize)]
struct Iteration {
    /// The iteration's number.
    index: usize,
    /// Which rung ran first.
    order: String,
    /// The host's load average.
    load_average: f64,
    /// The table's own construction, in milliseconds.
    table_build_ms: f64,
    /// The filter's own construction, in milliseconds.
    filter_build_ms: f64,
    /// The table's own load, in milliseconds.
    floor_table_ms: f64,
    /// The filter's own load, in milliseconds.
    floor_filter_ms: f64,
    /// The probe as it is, in milliseconds.
    candidate_table_ms: f64,
    /// The probe with the filter, in milliseconds.
    candidate_filter_ms: f64,
    /// The probe with the filter compacting the rows in front of it.
    candidate_compacted_ms: f64,
    /// The probe over the survivors alone, in milliseconds.
    candidate_survivors_ms: f64,
    /// The engine's own probe, in milliseconds.
    engine_probe_ms: f64,
    /// Whether the two candidates' rows are the same, row for row.
    rows_agree: bool,
    /// Whether the engine's own probe's rows are the candidates' rows.
    engine_agrees: bool,
    /// How many rows the filter let through.
    rows_through: usize,
}

/// The build side's file, beside the probe side's.
///
/// It is the house's own rule: the probe side is `lineitem`, the build side is
/// `orders`, and the suffixes ride along so that the row-group and the
/// single-file shapes can be measured over the same pair.
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
                println!("usage: key_filter_probe --data <parquet> [--out <report.json>]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let path = path.unwrap_or_else(|| PathBuf::from("data/lineitem.parquet"));
    let out = out.unwrap_or_else(|| PathBuf::from("bench-results/key-filter-run1.json"));
    Ok((path, out))
}

/// What an iteration needs: the probe side's source, the engine's own table,
/// the instrument's table and filter, and the build side's keys.
struct Campaign<'a> {
    /// The probe side's source.
    source: &'a ParquetSource,
    /// The engine's own table, for the control.
    engine_table: &'a BuildTable,
    /// The instrument's own table: the probe as it is.
    table: &'a Table,
    /// The instrument's own filter: one bit a key.
    bits: &'a [u64],
    /// The build side's keys.
    build_keys: &'a [i64],
    /// The base the table addresses from.
    base: i64,
    /// How many keys the table addresses.
    width: usize,
    /// The probe side's key column's index.
    probe_index: usize,
    /// The kernel flavor the engine reads with.
    flavor: Flavor,
}

/// One iteration: the rungs, in the iteration's order.
fn measure_iteration(
    campaign: &Campaign<'_>,
    index: usize,
    order: &[usize; 3],
) -> Result<Iteration, Box<dyn std::error::Error>> {
    let load = load_average();

    // The column, materialised afresh: the reader has just written it.
    // **It is not copied**: a copy would be a buffer the process kept warm,
    // and the probe would be measured on that instead of on the column the
    // reader wrote.
    let fresh = campaign.source.read(
        &[campaign.probe_index],
        &PushdownPlan::default(),
        campaign.flavor,
        1,
    )?;
    let fresh_chunk = fresh.chunk(PROBE_KEY).ok_or("the probe side's key is there")?;
    let fresh_keys = KeyColumn::from_chunk(fresh_chunk);
    let values = fresh_keys.integer_values().ok_or("the probe side's key is plain integers")?;
    let rows = 0..u32::try_from(fresh.len()).unwrap_or(0);

    // The three candidates, back to back, in the iteration's order.
    let mut table_ms = 0.0_f64;
    let mut filter_ms = 0.0_f64;
    let mut compacted_ms = 0.0_f64;
    let mut table_rows: Vec<u32> = Vec::new();
    let mut filter_rows: Vec<u32> = Vec::new();
    let mut compacted_rows: Vec<u32> = Vec::new();
    for rung in order {
        let started = Instant::now();
        match rung {
            0 => {
                table_rows = candidate_table(campaign.table, &fresh_keys, rows.clone());
                table_ms = ms(started.elapsed());
            }
            1 => {
                compacted_rows =
                    candidate_compacted(campaign.table, campaign.bits, &fresh_keys, rows.clone());
                compacted_ms = ms(started.elapsed());
            }
            _ => {
                filter_rows =
                    candidate_filter(campaign.table, campaign.bits, &fresh_keys, rows.clone());
                filter_ms = ms(started.elapsed());
            }
        }
    }
    let rows_agree = table_rows == filter_rows && table_rows == compacted_rows;

    // The engine's own probe, over the same column: the rung that says the
    // candidates are the engine's probe.
    let started = Instant::now();
    let engine_rows = probe_rows(campaign.engine_table, &fresh_keys, rows.clone());
    let engine_probe_ms = ms(started.elapsed());
    let engine_agrees = engine_rows == compacted_rows;

    // The survivors alone, and the probe over them: **the prize's other
    // half** -- the join the engine would be left with if the filter kept the
    // rows out of it.
    let survivor_rows = survivors(campaign.bits, campaign.base, values);
    let started = Instant::now();
    let survivor_out = candidate_table(campaign.table, &fresh_keys, survivor_rows.iter().copied());
    let candidate_survivors_ms = ms(started.elapsed());
    assert_eq!(survivor_out, engine_rows, "the survivors' rows are the probe's rows");

    // The two constructions: the table's, and the filter's.
    let started = Instant::now();
    let built = construct_table(campaign.build_keys, campaign.base, campaign.width);
    let table_build_ms = ms(started.elapsed());
    assert_eq!(built.len(), campaign.table.counts.len(), "the built table is the table's size");
    let started = Instant::now();
    let built_bits = construct_filter(campaign.build_keys, campaign.base, campaign.width);
    let filter_build_ms = ms(started.elapsed());
    assert_eq!(built_bits.len(), campaign.bits.len(), "the built filter is the filter's size");

    // The floors: the table's own load, and the filter's own load.
    let started = Instant::now();
    floor_table(&campaign.table.counts, campaign.base, values);
    let floor_table_ms = ms(started.elapsed());
    let started = Instant::now();
    floor_filter(campaign.bits, campaign.base, values);
    let floor_filter_ms = ms(started.elapsed());

    let order_name = order
        .iter()
        .map(|rung| CANDIDATE_NAMES.get(*rung).copied().unwrap_or("unknown"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(Iteration {
        index,
        order: order_name,
        load_average: load,
        table_build_ms,
        filter_build_ms,
        floor_table_ms,
        floor_filter_ms,
        candidate_table_ms: table_ms,
        candidate_filter_ms: filter_ms,
        candidate_compacted_ms: compacted_ms,
        candidate_survivors_ms,
        engine_probe_ms,
        rows_agree,
        engine_agrees,
        rows_through: survivor_rows.len(),
    })
}

/// The campaign: the iterations, in order.
fn campaign(campaign: &Campaign<'_>) -> Result<Vec<Iteration>, Box<dyn std::error::Error>> {
    let mut iterations = Vec::with_capacity(ITERATIONS);
    let mut rows_through_first: Option<usize> = None;
    for (index, order) in ORDERS.iter().enumerate() {
        let iteration = measure_iteration(campaign, index, order)?;
        let rows_through = rows_through_first.get_or_insert(iteration.rows_through);
        assert_eq!(
            iteration.rows_through, *rows_through,
            "the filter lets the same number of rows through every iteration"
        );
        iterations.push(iteration);
    }
    Ok(iterations)
}

/// The report: the campaign's own figures, and the verdicts.
fn the_report(
    iterations: &[Iteration],
    build_keys: &[i64],
    bits: &[u64],
    engine_table: &BuildTable,
    table_bytes: usize,
    base: i64,
    width: usize,
) -> serde_json::Map<String, serde_json::Value> {
    // The report.
    let series =
        |pick: fn(&Iteration) -> f64| -> Vec<f64> { iterations.iter().map(pick).collect() };
    let mut report = serde_json::Map::new();
    report.insert("probe".to_owned(), serde_json::json!("key-filter"));
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
    report.insert("build_keys".to_owned(), serde_json::json!(build_keys.len()));
    report.insert(
        "probe_rows".to_owned(),
        serde_json::json!(iterations.first().map_or(0, |i| i.rows_through)),
    );
    report.insert("table_bytes".to_owned(), serde_json::json!(table_bytes));
    report.insert("filter_bytes".to_owned(), serde_json::json!(bits.len() * 8));
    report.insert("base".to_owned(), serde_json::json!(base));
    report.insert("width".to_owned(), serde_json::json!(width));
    report.insert("expected_rows".to_owned(), serde_json::json!(EXPECTED_ROWS));
    report.insert("engine_rows".to_owned(), serde_json::json!(engine_table.rows()));
    for (name, values) in [
        ("table_build_ms", series(|i| i.table_build_ms)),
        ("filter_build_ms", series(|i| i.filter_build_ms)),
        ("floor_table_ms", series(|i| i.floor_table_ms)),
        ("floor_filter_ms", series(|i| i.floor_filter_ms)),
        ("candidate_table_ms", series(|i| i.candidate_table_ms)),
        ("candidate_filter_ms", series(|i| i.candidate_filter_ms)),
        ("candidate_compacted_ms", series(|i| i.candidate_compacted_ms)),
        ("candidate_survivors_ms", series(|i| i.candidate_survivors_ms)),
        ("engine_probe_ms", series(|i| i.engine_probe_ms)),
    ] {
        report.insert(
            name.to_owned(),
            serde_json::json!({
                "median": median(&values),
                "min": values.iter().copied().fold(f64::INFINITY, f64::min),
                "max": values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "samples": values,
            }),
        );
    }
    // The filter's verdict: the paired difference, in the same iteration.
    //
    // **Three verdicts, three shapes**, each one the paired difference in the
    // same iteration, back to back, the order alternating:
    //
    // | the verdict | what it says |
    // |---|---|
    // | **`the_filter_s_verdict_ms`** | the filter in front of the table: the filter's load and its branch, less the table's load it skips |
    // | **`the_compaction_s_verdict_ms`** | the filter compacting the rows: the same filter, and the survivors probed alone |
    // | **`the_prize_ms`** | the engine's own probe, less its own probe over the survivors: **the whole join less what the filter would leave** |
    let verdict = |report: &mut serde_json::Map<String, serde_json::Value>,
                   name: &str,
                   differences: &[f64]| {
        report.insert(
            name.to_owned(),
            serde_json::json!({
                "median": median(differences),
                "min": differences.iter().copied().fold(f64::INFINITY, f64::min),
                "max": differences.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "samples": differences,
            }),
        );
    };
    let differences: Vec<f64> = iterations
        .iter()
        .filter(|i| adjacent(&i.order, "table", "filter"))
        .map(|i| i.candidate_filter_ms - i.candidate_table_ms)
        .collect();
    verdict(&mut report, "the_filter_s_verdict_ms", &differences);
    let differences: Vec<f64> = iterations
        .iter()
        .filter(|i| adjacent(&i.order, "table", "compacted"))
        .map(|i| i.candidate_compacted_ms - i.candidate_table_ms)
        .collect();
    verdict(&mut report, "the_compaction_s_verdict_ms", &differences);
    let differences: Vec<f64> =
        iterations.iter().map(|i| i.candidate_table_ms - i.candidate_survivors_ms).collect();
    verdict(&mut report, "the_prize_ms", &differences);
    report.insert(
        "the_rows_agree".to_owned(),
        serde_json::json!(iterations.iter().all(|i| i.rows_agree)),
    );
    report.insert(
        "the_engine_agrees".to_owned(),
        serde_json::json!(iterations.iter().all(|i| i.engine_agrees)),
    );
    report.insert(
        "rows_through".to_owned(),
        serde_json::json!(iterations.iter().map(|i| i.rows_through).collect::<Vec<_>>()),
    );
    report.insert("the_iterations".to_owned(), serde_json::json!(iterations));

    report
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (data, out) = arguments()?;

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

    // The build side's keys, and the range the table addresses.
    let pushdown = build_pushdown(&build_source);
    let build_key_index =
        build_source.column_index(BUILD_KEY).ok_or_else(|| format!("no {BUILD_KEY} column"))?;
    let build_dataset = build_source.read(&[build_key_index], &pushdown, flavor, 1)?;
    let build_chunk = build_dataset.chunk(BUILD_KEY).ok_or("the build side's key is there")?;
    let build_keys = collect_keys(build_chunk);
    let (base, width) = build_range(&build_source);
    drop(build_dataset);

    // The instrument's own table and filter, over the build side's keys.
    let counts = counts(&build_keys, base, width);
    let bits = bits(&build_keys, base, width);

    // The engine's own table, for the cross-check.
    let engine_table = build_table(&build_source, spec, flavor, 1)?;
    assert_eq!(
        engine_table.rows(),
        build_keys.len(),
        "the engine's table holds the build side's keys"
    );

    // The probe side's key column's index.
    let probe_index =
        source.column_index(PROBE_KEY).ok_or_else(|| format!("no {PROBE_KEY} column"))?;

    // The instrument's own table, without the filter: the probe as it is.
    let table_bytes = counts.len();
    let table = Table { counts, base, overflow: None };

    let context = Campaign {
        source: &source,
        engine_table: &engine_table,
        table: &table,
        bits: &bits,
        build_keys: &build_keys,
        base,
        width,
        probe_index,
        flavor,
    };
    let iterations = campaign(&context)?;

    let report =
        the_report(&iterations, &build_keys, &bits, &engine_table, table_bytes, base, width);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    println!("wrote {}", out.display());
    Ok(())
}
