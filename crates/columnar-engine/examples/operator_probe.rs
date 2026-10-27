//! The operator probe -- the join's probe and the sort, taken apart.
//!
//! This is the measurement behind spec 021, and a diagnostic, not part of the
//! engine.
//!
//! ```text
//! cargo run --release -p columnar-engine --example operator_probe -- \
//!     --data data/lineitem.parquet --build data/orders.parquet --out report.json
//! ```
//!
//! # The question
//!
//! Two operators were published in spec 020 and both are behind the
//! incumbents:
//!
//! | query | engine | duckdb | polars | datafusion |
//! |---|---|---|---|---|
//! | `join-orders` cold | 216.1 ms | **97.0 ms** | 231.6 ms | 177.3 ms |
//! | `sort-topk` cold | 61.6 ms | **40.0 ms** | 1460.6 ms | 41.3 ms |
//! | `sort-topk` resident | 23.6 ms | **4.0 ms** | 1452.7 ms | 4.5 ms |
//!
//! The plan output already says where the join's time goes: the join stage is
//! 128.1 ms of a 216 ms query, and the scan is 79.6 ms. The scan is Arrow's
//! own decode, and the existing `materialised_column_probe` measured that a
//! different encoding would be *slower*, so the scan is at the floor of what
//! this file format allows.
//!
//! What is left is the engine's own code, and this probe takes it apart:
//!
//! | step | what it measures |
//! |---|---|
//! | **read** | the floor: reading the key column, one row at a time |
//! | **engine** | the engine's own probe, exactly as it runs today |
//! | **prefilter** | the same table, behind a bitmap that rejects most probes |
//! | **int-table** | an open-addressed table over the key's bits |
//! | **int-table + prefilter** | both |
//!
//! Every candidate is **checked against the engine's own output**, row for
//! row, before it is timed. A candidate that is wrong is not a candidate.
//!
//! # Why a prefilter
//!
//! The build side holds 216,345 keys and the probe side asks 6,000,000
//! questions, of which **96.4% have the answer "no"**. Every one of those
//! six million questions costs a hash and a probe into a table that is a few
//! megabytes -- a cache miss, which is the expensive part. A bitmap of one
//! bit per build key is small enough to sit in the cache, and it turns most
//! of those questions into a single bit test.
//!
//! # Why an open-addressed table
//!
//! The engine's table is a `HashMap` over an enum. The enum's discriminant
//! is compared on every lookup, the hash map's control bytes are scanned a
//! group at a time, and the key is stored as 32 bytes whether it is a number
//! or a string. A join on a numeric key needs 8 bytes of key and nothing
//! else.

// A diagnostic: it indexes deliberately, to measure the cost of an indexed
// read against the cost of a checked one.
#![allow(clippy::indexing_slicing, clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::path::Path;
use std::time::Instant;

use columnar_core::{ComparisonOp, Encoding, ScalarValue};
use columnar_engine::operators::key::{KeyColumn, KeyValue};
use columnar_engine::operators::{join, sort};
use columnar_engine::plan::JoinSpec;
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::source::ParquetSource;
use columnar_kernels::Flavor;

/// The date range the build side is filtered by.
const BUILD_FROM: &str = "1994-01-01";
/// The date range the build side is filtered by.
const BUILD_TO: &str = "1995-01-01";

/// The join's probe key.
const PROBE_KEY: &str = "l_orderkey";
/// The build side's key.
const BUILD_KEY: &str = "o_orderkey";
/// The build side's filter column.
const BUILD_DATE: &str = "o_orderdate";
/// The sort's key.
const SORT_KEY: &str = "l_extendedprice";

/// How many timed passes each measurement takes.
const PASSES: usize = 5;

/// How many rows the sort keeps.
const LIMIT: usize = 10;

/// The hash the join's table uses.
///
/// This is `FxHash`'s arithmetic -- a rotate, an exclusive-or and a
/// multiply -- and it is the same arithmetic the engine's own table uses, so
/// the two tables are compared on equal terms.
#[derive(Debug, Default, Clone, Copy)]
struct ProbeHasher {
    /// The running hash.
    state: u64,
}

impl ProbeHasher {
    /// The multiplier: the 64-bit golden ratio's odd companion.
    const SEED: u64 = 0x517c_c1b7_2722_0a95;
}

impl Hasher for ProbeHasher {
    fn finish(&self) -> u64 {
        self.state
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks_exact(8) {
            let mut buffer = [0_u8; 8];
            buffer.copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(buffer));
        }
        let rest = bytes.chunks_exact(8).remainder();
        if !rest.is_empty() {
            let mut buffer = [0_u8; 8];
            buffer[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(buffer));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.state = (self.state.rotate_left(5) ^ value).wrapping_mul(Self::SEED);
    }
}

/// The table the probe builds: the same arithmetic, as a `HashMap`.
type ProbeMap = HashMap<u64, u32, BuildHasherDefault<ProbeHasher>>;

/// The hash of a key.
fn hash_key(key: i64) -> u64 {
    let mut hasher = ProbeHasher::default();
    hasher.write_u64(key as u64);
    hasher.finish()
}

/// The Fibonacci hash of a key: one multiply, no state.
///
/// A multiply followed by a shift spreads the low bits of a key over the
/// whole word, which is what a filter needs: the filter's index must not
/// depend on how the key was numbered.
fn fib_hash(key: i64) -> u64 {
    (key as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// A bitmap of the keys the table holds, indexed by the Fibonacci hash.
///
/// One bit per build key, sized so the bitmap stays in the first-level
/// cache. A probe that finds a clear bit knows the answer is "no" without
/// touching the table at all.
#[derive(Debug)]
struct FibFilter {
    /// The bits.
    words: Vec<u64>,
    /// The mask for a word index.
    mask: usize,
}

impl FibFilter {
    /// Builds the filter over `keys`, with `bits` bits.
    fn build(keys: &[i64], bits: usize) -> Self {
        let bits = bits.next_power_of_two();
        let mut words = vec![0_u64; bits / 64];
        let mask = words.len() - 1;
        for key in keys {
            let index = fib_hash(*key) as usize & mask;
            words[index / 64] |= 1_u64 << (index % 64);
        }
        Self { words, mask }
    }

    /// Whether the key might be in the table.
    fn may_hold(&self, key: i64) -> bool {
        let index = fib_hash(key) as usize & self.mask;
        self.words[index / 64] & (1_u64 << (index % 64)) != 0
    }
}

/// An open-addressed table over a numeric key's bits.
///
/// The slots hold the key's bits and the number of build rows that carry it.
/// A free slot holds [`IntTable::EMPTY`].
#[derive(Debug)]
struct IntTable {
    /// The key bits per slot.
    keys: Vec<u64>,
    /// The count per slot.
    counts: Vec<u32>,
    /// The mask for a slot index.
    mask: usize,
    /// Whether the slot index comes from the Fibonacci hash.
    fibonacci: bool,
}

impl IntTable {
    /// The marker of a free slot.
    const EMPTY: u64 = u64::MAX;

    /// Builds the table over `keys`, with the Fibonacci hash.
    fn build(keys: &[i64]) -> Self {
        Self::build_with(keys, true)
    }

    /// Builds the table over `keys`, with either hash.
    fn build_with(keys: &[i64], fibonacci: bool) -> Self {
        let capacity = (keys.len().saturating_mul(2)).max(16).next_power_of_two();
        let mut table = Self {
            keys: vec![Self::EMPTY; capacity],
            counts: vec![0; capacity],
            mask: capacity - 1,
            fibonacci,
        };
        for key in keys {
            let bits = *key as u64;
            let mut slot = table.slot(*key);
            loop {
                if table.keys[slot] == Self::EMPTY {
                    table.keys[slot] = bits;
                    table.counts[slot] = 1;
                    break;
                }
                if table.keys[slot] == bits {
                    table.counts[slot] = table.counts[slot].saturating_add(1);
                    break;
                }
                slot = (slot + 1) & table.mask;
            }
        }
        table
    }

    /// The slot a key lands in.
    fn slot(&self, key: i64) -> usize {
        let hash = if self.fibonacci { fib_hash(key) } else { hash_key(key) };
        hash as usize & self.mask
    }

    /// How many build rows carry `key`.
    fn count(&self, key: i64) -> u32 {
        let bits = key as u64;
        let mut slot = self.slot(key);
        loop {
            let held = self.keys[slot];
            if held == Self::EMPTY {
                return 0;
            }
            if held == bits {
                return self.counts[slot];
            }
            slot = (slot + 1) & self.mask;
        }
    }
}

/// Keeps the key column alive for as long as the probe side does.
struct OwnedProbe {
    /// The dataset the key column borrows from.
    dataset: columnar_engine::Dataset,
    /// The rows the probe walks.
    rows: Vec<u32>,
    /// The probe key's own integers.
    plain: Vec<i64>,
}

/// Reads the probe side's key columns: the join's key and the sort's key.
fn read_probe_side(path: &Path) -> OwnedProbe {
    let source = ParquetSource::open(path).expect("the probe side opens");
    let key = source.column_index(PROBE_KEY).expect("the probe side has the probe key");
    let price = source.column_index(SORT_KEY).expect("the probe side has the sort key");
    let dataset = source
        .read(&[key, price], &PushdownPlan::new(), Flavor::CppSimd, 1)
        .expect("the probe side reads");
    let rows = (0..source.rows()).map(|row| u32::try_from(row).unwrap_or(u32::MAX)).collect();
    let chunk = dataset.chunk(PROBE_KEY).expect("the probe side's key chunk");
    let plain = match &chunk.encoding {
        Encoding::Int64 { values } => values.clone(),
        Encoding::Int64Dict { dict, indices } => {
            indices.iter().map(|index| dict[usize::try_from(*index).unwrap_or(0)]).collect()
        }
        other => panic!("the probe side's key is not an integer: {other:?}"),
    };
    OwnedProbe { dataset, rows, plain }
}

/// Reads the build side's keys, with the build side's own filter applied.
fn read_build_side(path: &Path) -> Vec<i64> {
    let source = ParquetSource::open(path).expect("the build side opens");
    let key = source.column_index(BUILD_KEY).expect("the build side has its key");
    let date = source.column_index(BUILD_DATE).expect("the build side has its date column");
    let mut predicate = Pushdown::new(
        date,
        BUILD_DATE,
        ComparisonOp::GtEq,
        ScalarValue::Utf8(BUILD_FROM.to_owned()),
    );
    predicate.and(ComparisonOp::Lt, ScalarValue::Utf8(BUILD_TO.to_owned()));
    let mut pushdown = PushdownPlan::new();
    pushdown.push(predicate);
    let dataset = source.read(&[key], &pushdown, Flavor::CppSimd, 1).expect("the build side reads");
    let chunk = dataset.chunk(BUILD_KEY).expect("the build side's key chunk");
    match &chunk.encoding {
        Encoding::Int64 { values } => values.clone(),
        Encoding::Int64Dict { dict, indices } => {
            indices.iter().map(|index| dict[usize::try_from(*index).unwrap_or(0)]).collect()
        }
        other => panic!("the build side's key is not an integer: {other:?}"),
    }
}

/// Builds the engine's own join table, over the same build side and the same
/// filter.
fn engine_table(path: &Path) -> join::BuildTable {
    let spec = JoinSpec {
        build_path: path.to_path_buf(),
        probe_key: PROBE_KEY.to_owned(),
        build_key: BUILD_KEY.to_owned(),
        build_filter: vec![
            columnar_core::Predicate::new(
                BUILD_DATE,
                ComparisonOp::GtEq,
                ScalarValue::Utf8(BUILD_FROM.to_owned()),
            ),
            columnar_core::Predicate::new(
                BUILD_DATE,
                ComparisonOp::Lt,
                ScalarValue::Utf8(BUILD_TO.to_owned()),
            ),
        ],
    };
    let source = ParquetSource::open(path).expect("the build side opens");
    join::build_table(&source, &spec, Flavor::CppSimd, 1).expect("the build table builds")
}

/// A table addressed by the key itself.
///
/// The key space is dense -- the build side's keys run from 1 to 6,000,000 --
/// so the table does not have to hash at all: the key *is* the index. The
/// price is space: the table is as wide as the key range, not as wide as the
/// number of keys.
#[derive(Debug)]
struct DirectTable {
    /// The counts, indexed by the key less the base.
    counts: Vec<u32>,
    /// The smallest key.
    base: i64,
}

impl DirectTable {
    /// Builds the table over `keys`.
    fn build(keys: &[i64]) -> Self {
        let base = keys.iter().copied().min().unwrap_or(0);
        let top = keys.iter().copied().max().unwrap_or(0);
        let width = usize::try_from(top.saturating_sub(base)).unwrap_or(0).saturating_add(1);
        let mut counts = vec![0_u32; width];
        for key in keys {
            let index = usize::try_from(key.saturating_sub(base)).unwrap_or(0);
            if let Some(count) = counts.get_mut(index) {
                *count = count.saturating_add(1);
            }
        }
        Self { counts, base }
    }

    /// How many build rows carry `key`.
    ///
    /// A key **below** the base is not in the table: the index would be
    /// negative, and a table that wrapped it round would answer with the
    /// base key's count instead of with nothing.
    fn count(&self, key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        self.counts.get(index).copied().unwrap_or(0)
    }

    /// How many bytes the table holds.
    fn bytes(&self) -> usize {
        self.counts.len() * std::mem::size_of::<u32>()
    }
}

/// A table addressed by the key itself, one byte a count.
///
/// The same structure as [`DirectTable`], but the counts are bytes: a build
/// side whose key repeats more than 255 times would not fit, and the probe
/// checks that it does not.
#[derive(Debug)]
struct DirectBytes {
    /// The counts, indexed by the key less the base.
    counts: Vec<u8>,
    /// The smallest key.
    base: i64,
}

impl DirectBytes {
    /// Builds the table over `keys`.
    fn build(keys: &[i64]) -> Self {
        let base = keys.iter().copied().min().unwrap_or(0);
        let top = keys.iter().copied().max().unwrap_or(0);
        let width = usize::try_from(top.saturating_sub(base)).unwrap_or(0).saturating_add(1);
        let mut counts = vec![0_u8; width];
        for key in keys {
            let index = usize::try_from(key.saturating_sub(base)).unwrap_or(0);
            if let Some(count) = counts.get_mut(index) {
                *count = count.saturating_add(1);
            }
        }
        Self { counts, base }
    }

    /// How many build rows carry `key`.
    ///
    /// A key **below** the base is not in the table, for the same reason as
    /// in [`DirectTable::count`].
    fn count(&self, key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        u32::from(self.counts.get(index).copied().unwrap_or(0))
    }

    /// How many bytes the table holds.
    fn bytes(&self) -> usize {
        self.counts.len()
    }
}

/// The measured steps, each a median over [`PASSES`] passes.
#[derive(Debug, Default)]
struct Measurements {
    /// The floor: reading the key column.
    read: Vec<f64>,
    /// The floor: reading the key column's own integers.
    read_plain: Vec<f64>,
    /// The engine's own probe.
    engine: Vec<f64>,
    /// The engine's table behind a bitmap.
    prefilter: Vec<f64>,
    /// An open-addressed table over the probe's hash.
    int_table: Vec<f64>,
    /// An open-addressed table over the Fibonacci hash.
    int_table_fib: Vec<f64>,
    /// The Fibonacci table behind a small bitmap.
    int_table_fib_filter: Vec<f64>,
    /// The Fibonacci table behind a larger bitmap.
    int_table_fib_filter_big: Vec<f64>,
    /// Both.
    int_table_prefilter: Vec<f64>,
    /// A table addressed by the key itself.
    direct: Vec<f64>,
    /// A table addressed by the key itself, one byte a count.
    direct_bytes: Vec<f64>,
}

/// The median of a measurement's passes.
fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

/// Times a pass, in milliseconds.
fn time_pass(mut body: impl FnMut()) -> f64 {
    let started = Instant::now();
    body();
    started.elapsed().as_secs_f64() * 1000.0
}

/// The probe's rows, and the keys they carry.
struct ProbeRows<'a, 'b> {
    /// The key column.
    keys: &'a KeyColumn<'b>,
    /// The rows the probe walks.
    rows: &'a [u32],
    /// The key column's own integers.
    plain: &'a [i64],
}

/// The floor: reading the key column, one row at a time.
fn measure_read(probe: &ProbeRows<'_, '_>) -> f64 {
    time_pass(|| {
        let mut total = 0_i64;
        for row in probe.rows {
            if let Some(KeyValue::Int(value)) =
                probe.keys.value_at(usize::try_from(*row).unwrap_or(0))
            {
                total = total.wrapping_add(value);
            }
        }
        std::hint::black_box(total);
    })
}

/// The floor: reading the key column's own integers.
fn measure_read_plain(probe: &ProbeRows<'_, '_>) -> f64 {
    time_pass(|| {
        let mut total = 0_i64;
        for row in probe.rows {
            total = total.wrapping_add(probe.plain[usize::try_from(*row).unwrap_or(0)]);
        }
        std::hint::black_box(total);
    })
}

/// The engine's own probe.
fn measure_engine_probe(table: &join::BuildTable, probe: &ProbeRows<'_, '_>) -> f64 {
    time_pass(|| {
        let out = join::probe(table, probe.keys, probe.rows);
        std::hint::black_box(out.len());
    })
}

/// The engine's table behind a bitmap.
fn rows_prefilter(map: &ProbeMap, filter: &FibFilter, probe: &ProbeRows<'_, '_>) -> Vec<u32> {
    let mut out = Vec::new();
    for row in probe.rows {
        let Some(KeyValue::Int(value)) = probe.keys.value_at(usize::try_from(*row).unwrap_or(0))
        else {
            continue;
        };
        if !filter.may_hold(value) {
            continue;
        }
        let count = map.get(&(value as u64)).copied().unwrap_or(0);
        for _ in 0..count {
            out.push(*row);
        }
    }
    out
}

/// An open-addressed table, with an optional bitmap in front of it.
fn rows_int_table(
    table: &IntTable,
    filter: Option<&FibFilter>,
    probe: &ProbeRows<'_, '_>,
) -> Vec<u32> {
    let mut out = Vec::new();
    for row in probe.rows {
        let Some(KeyValue::Int(value)) = probe.keys.value_at(usize::try_from(*row).unwrap_or(0))
        else {
            continue;
        };
        if let Some(filter) = filter {
            if !filter.may_hold(value) {
                continue;
            }
        }
        let count = table.count(value);
        for _ in 0..count {
            out.push(*row);
        }
    }
    out
}

/// A table addressed by the key itself.
fn rows_direct(table: &DirectTable, probe: &ProbeRows<'_, '_>) -> Vec<u32> {
    let mut out = Vec::new();
    for row in probe.rows {
        let Some(KeyValue::Int(value)) = probe.keys.value_at(usize::try_from(*row).unwrap_or(0))
        else {
            continue;
        };
        let count = table.count(value);
        for _ in 0..count {
            out.push(*row);
        }
    }
    out
}

/// A table addressed by the key itself, one byte a count.
fn rows_direct_bytes(table: &DirectBytes, probe: &ProbeRows<'_, '_>) -> Vec<u32> {
    let mut out = Vec::new();
    for row in probe.rows {
        let Some(KeyValue::Int(value)) = probe.keys.value_at(usize::try_from(*row).unwrap_or(0))
        else {
            continue;
        };
        let count = table.count(value);
        for _ in 0..count {
            out.push(*row);
        }
    }
    out
}

/// Checks a candidate against the engine's own output, row for row.
///
/// A candidate that is wrong is not a candidate, so the check comes **before**
/// the timing: every number the probe publishes is the number for work that
/// produces the engine's answer.
fn check_candidate(name: &str, expected: &[u32], found: &[u32]) {
    assert_eq!(
        found.len(),
        expected.len(),
        "the {name} candidate produced {} rows, the engine produced {}",
        found.len(),
        expected.len()
    );
    for (index, (left, right)) in expected.iter().zip(found).enumerate() {
        assert_eq!(left, right, "the {name} candidate differs from the engine at row {index}");
    }
}

/// Times a candidate that produces rows.
fn time_rows(body: impl Fn() -> Vec<u32>) -> f64 {
    time_pass(|| {
        let rows = body();
        std::hint::black_box(rows.len());
    })
}

/// Runs every measurement, `PASSES` times each.
fn measure(
    keys: &KeyColumn<'_>,
    rows: &[u32],
    plain: &[i64],
    build_keys: &[i64],
    build_path: &Path,
) -> Measurements {
    let mut out = Measurements::default();
    let engine = engine_table(build_path);
    let map: ProbeMap = build_keys.iter().copied().map(|key| (key as u64, 1)).collect();
    let filter = FibFilter::build(build_keys, build_keys.len().saturating_mul(8).max(1 << 12));
    let table = IntTable::build_with(build_keys, false);
    let fib_table = IntTable::build(build_keys);
    let fib_filter = FibFilter::build(build_keys, 1 << 13);
    let fib_filter_big = FibFilter::build(build_keys, 1 << 18);
    let direct = DirectTable::build(build_keys);
    let direct_bytes = DirectBytes::build(build_keys);
    let probe = ProbeRows { keys, rows, plain };

    validate_candidates(
        &probe,
        &engine,
        &map,
        &filter,
        &table,
        &fib_table,
        &fib_filter,
        &fib_filter_big,
        &direct,
        &direct_bytes,
    );

    for _ in 0..PASSES {
        out.read.push(measure_read(&probe));
        out.read_plain.push(measure_read_plain(&probe));
        out.engine.push(measure_engine_probe(&engine, &probe));
        out.prefilter.push(time_rows(|| rows_prefilter(&map, &filter, &probe)));
        out.int_table.push(time_rows(|| rows_int_table(&table, None, &probe)));
        out.int_table_fib.push(time_rows(|| rows_int_table(&fib_table, None, &probe)));
        out.int_table_fib_filter
            .push(time_rows(|| rows_int_table(&fib_table, Some(&fib_filter), &probe)));
        out.int_table_fib_filter_big
            .push(time_rows(|| rows_int_table(&fib_table, Some(&fib_filter_big), &probe)));
        out.int_table_prefilter.push(time_rows(|| rows_int_table(&table, Some(&filter), &probe)));
        out.direct.push(time_rows(|| rows_direct(&direct, &probe)));
        out.direct_bytes.push(time_rows(|| rows_direct_bytes(&direct_bytes, &probe)));
    }
    out
}

/// Checks every candidate against the engine's own output, row for row.
///
/// The check runs **once**, before any candidate is timed. A candidate that
/// produces a different answer is not a candidate: it is a different operator,
/// and the number it produces is not a number for this one.
#[allow(clippy::too_many_arguments)]
fn validate_candidates(
    probe: &ProbeRows<'_, '_>,
    engine: &join::BuildTable,
    map: &ProbeMap,
    filter: &FibFilter,
    table: &IntTable,
    fib_table: &IntTable,
    fib_filter: &FibFilter,
    fib_filter_big: &FibFilter,
    direct: &DirectTable,
    direct_bytes: &DirectBytes,
) {
    let expected = join::probe(engine, probe.keys, probe.rows);
    check_candidate("prefilter", &expected, &rows_prefilter(map, filter, probe));
    check_candidate("int-table", &expected, &rows_int_table(table, None, probe));
    check_candidate("int-table-fib", &expected, &rows_int_table(fib_table, None, probe));
    check_candidate(
        "int-table-fib-filter",
        &expected,
        &rows_int_table(fib_table, Some(fib_filter), probe),
    );
    check_candidate(
        "int-table-fib-filter-big",
        &expected,
        &rows_int_table(fib_table, Some(fib_filter_big), probe),
    );
    check_candidate("int-table-prefilter", &expected, &rows_int_table(table, Some(filter), probe));
    check_candidate("direct", &expected, &rows_direct(direct, probe));
    check_candidate("direct-bytes", &expected, &rows_direct_bytes(direct_bytes, probe));
}

/// The sort's measurements: the floor, and the sort itself.
#[derive(Debug, Default)]
struct SortMeasurements {
    /// The floor: reading the key column.
    read: Vec<f64>,
    /// The floor without the validity check.
    read_plain: Vec<f64>,
    /// The engine's bounded sort.
    bounded: Vec<f64>,
    /// The engine's bounded sort over a plain read.
    bounded_plain: Vec<f64>,
}

/// Measures the sort.
fn measure_sort(dataset: &columnar_engine::Dataset, rows: &[u32]) -> SortMeasurements {
    let mut out = SortMeasurements::default();
    let chunk = dataset.chunk(SORT_KEY).expect("the sort's key chunk");
    let keys = KeyColumn::from_chunk(chunk);
    let plain: Vec<f64> = match &chunk.encoding {
        Encoding::Float64 { values } => values.clone(),
        other => panic!("the sort's key is not a plain float: {other:?}"),
    };
    let positions: Vec<u32> =
        (0..rows.len()).map(|index| u32::try_from(index).unwrap_or(0)).collect();

    // The engine's own sort is the reference: the hand-written one is checked
    // against it, row for row, before either of them is timed.
    let expected = sort::sort_rows(positions.clone(), &keys, true, Some(LIMIT));
    check_candidate("bounded-plain", &expected, &rows_bounded_plain(&plain, &positions));

    for _ in 0..PASSES {
        out.read.push(time_pass(|| {
            let mut total = 0.0_f64;
            for row in &positions {
                if let Some(KeyValue::Float(value)) =
                    keys.value_at(usize::try_from(*row).unwrap_or(0))
                {
                    total += value;
                }
            }
            std::hint::black_box(total);
        }));
        out.read_plain.push(time_pass(|| {
            let mut total = 0.0_f64;
            for row in &positions {
                total += plain[usize::try_from(*row).unwrap_or(0)];
            }
            std::hint::black_box(total);
        }));
        // The copy of the rows is the harness's, not the sort's, so it is made
        // **before** the clock starts.
        let mut input = Some(positions.clone());
        out.bounded.push(time_pass(|| {
            let rows = sort::sort_rows(
                input.take().expect("the input is there"),
                &keys,
                true,
                Some(LIMIT),
            );
            std::hint::black_box(rows.len());
        }));
        out.bounded_plain.push(time_rows(|| rows_bounded_plain(&plain, &positions)));
    }
    out
}

/// The bounded sort, written here rather than called: the best `limit` rows
/// of a column, by hand, with the key read as a plain value and no enum built
/// for it.
///
/// It is the cross-check of the engine's own sort, and the number for what the
/// sort costs when the only thing left in it is the comparison.
fn rows_bounded_plain(plain: &[f64], positions: &[u32]) -> Vec<u32> {
    // The best rows seen so far, in the engine's order: the best first, the
    // worst last. The worst is the one that would be dropped first.
    let mut best: [(f64, u32); LIMIT] = [(f64::NAN, u32::MAX); LIMIT];
    let mut filled = 0_usize;
    for row in positions {
        let entry = (plain[usize::try_from(*row).unwrap_or(0)], *row);
        if filled == LIMIT {
            if !is_better(entry, best[LIMIT - 1]) {
                continue;
            }
            best[LIMIT - 1] = entry;
        } else {
            best[filled] = entry;
            filled += 1;
        }
        // The worst of the ten is the last one: the array is kept sorted, so
        // the worst is at the end.
        let mut position = filled.saturating_sub(1);
        while position > 0 && is_better(entry, best[position - 1]) {
            best.swap(position - 1, position);
            position -= 1;
        }
    }
    best[..filled].iter().map(|(_, row)| *row).collect()
}

/// Whether `candidate` sorts before `other`.
///
/// The order is the engine's: the larger value first, and the earlier row
/// breaks a tie.
fn is_better(candidate: (f64, u32), other: (f64, u32)) -> bool {
    match candidate.0.total_cmp(&other.0) {
        Ordering::Greater => true,
        Ordering::Equal => candidate.1 < other.1,
        Ordering::Less => false,
    }
}

/// The arguments: the two files, and where to write the report.
struct Args {
    /// The probe side's file.
    data: String,
    /// The build side's file.
    build: String,
    /// Where the report goes.
    out: String,
}

/// Parses the arguments.
fn parse_args() -> Args {
    let mut data = "data/lineitem.parquet".to_owned();
    let mut build = "data/orders.parquet".to_owned();
    let mut out = "operator-probe.json".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().unwrap_or_default();
        match flag.as_str() {
            "--data" => data = value,
            "--build" => build = value,
            "--out" => out = value,
            _ => {}
        }
    }
    Args { data, build, out }
}

/// The build side's path, for the engine's own table.
fn main() {
    let args = parse_args();
    let build_path = Path::new(&args.build);
    let owned = read_probe_side(Path::new(&args.data));
    let keys =
        KeyColumn::from_chunk(owned.dataset.chunk(PROBE_KEY).expect("the probe side's key chunk"));
    let build_keys = read_build_side(build_path);
    let direct_bytes = DirectTable::build(&build_keys).bytes();
    let direct_byte_bytes = DirectBytes::build(&build_keys).bytes();
    let measurements = measure(&keys, &owned.rows, &owned.plain, &build_keys, build_path);
    let sort_measurements = measure_sort(&owned.dataset, &owned.rows);

    let report = serde_json::json!({
        "probe_side": {
            "file": args.data,
            "rows": owned.rows.len(),
            "probe_key_has_nulls": keys.has_nulls(),
        },
        "build_side": { "file": args.build, "keys": build_keys.len(), "direct_table_bytes": direct_bytes, "direct_byte_table_bytes": direct_byte_bytes },
        "join_probe_ms": {
            "read": median(&measurements.read),
            "read_plain": median(&measurements.read_plain),
            "engine": median(&measurements.engine),
            "prefilter": median(&measurements.prefilter),
            "int_table": median(&measurements.int_table),
            "int_table_fib": median(&measurements.int_table_fib),
            "int_table_fib_filter": median(&measurements.int_table_fib_filter),
            "int_table_fib_filter_big": median(&measurements.int_table_fib_filter_big),
            "int_table_prefilter": median(&measurements.int_table_prefilter),
            "direct": median(&measurements.direct),
            "direct_bytes": median(&measurements.direct_bytes),
        },
        "sort_ms": {
            "read": median(&sort_measurements.read),
            "read_plain": median(&sort_measurements.read_plain),
            "bounded": median(&sort_measurements.bounded),
            "bounded_plain": median(&sort_measurements.bounded_plain),
        },
        "samples": {
            "read": measurements.read,
            "read_plain": measurements.read_plain,
            "engine": measurements.engine,
            "prefilter": measurements.prefilter,
            "int_table": measurements.int_table,
            "int_table_fib": measurements.int_table_fib,
            "int_table_fib_filter": measurements.int_table_fib_filter,
            "int_table_fib_filter_big": measurements.int_table_fib_filter_big,
            "int_table_prefilter": measurements.int_table_prefilter,
            "direct": measurements.direct,
            "direct_bytes": measurements.direct_bytes,
            "sort_read": sort_measurements.read,
            "sort_read_plain": sort_measurements.read_plain,
            "sort_bounded": sort_measurements.bounded,
            "sort_bounded_plain": sort_measurements.bounded_plain,
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("the report serialises");
    std::fs::write(&args.out, &text).expect("the report is written");
    println!("{text}");
}
