//! The dictionary, kept or expanded.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind spec 022. It answers one question: **where does the engine's last
//! per-row-group cost come from?**
//!
//! The real Q6's **three**-predicate shape still rises by about **0.14 ms a
//! row group** across the three files, while the **two**-predicate shape is
//! flat. The two shapes differ by exactly one thing: the third predicate's
//! column, `l_quantity`, which the Parquet writer fell back to
//! dictionary-encoding -- 4,901 values a row group -- although the column was
//! written plain.
//!
//! The engine's reader asks the crate for **values**, so the crate expands
//! that dictionary into a row group's worth of values, and the engine compares
//! every one of them. The alternative is to ask for the **dictionary**: the
//! crate keeps the indices, and the engine compares the distinct values once
//! and maps the answer back through the indices -- which is what the engine
//! already does for `l_shipdate`, the one column whose file schema *is* a
//! dictionary.
//!
//! So the probe measures four shapes, on the same file, in the same process:
//!
//! | shape | what it is |
//! |---|---|
//! | `crate_expanded` | the crate's read, with the dictionaries expanded into values |
//! | `crate_kept` | the crate's read, with the dictionaries kept |
//! | `engine_expanded` | the crate's read, and the engine's comparison over the values |
//! | `engine_kept` | the crate's read, and the engine's comparison over the distinct values |
//!
//! The differences between the pairs are the engine's own work, and the three
//! files' rates say which of the two the per-row-group cost is.
//!
//! Every shape is read at the **batch size the engine would use for the
//! file**: the probe asks the engine for it, so its shapes are the engine's
//! shapes by construction rather than by a copy of the rule. The report
//! carries that batch size, the dictionary's **reuse** -- whether the crate
//! hands the same dictionary to two batches -- and the alternative's own
//! shape: the distinct-value comparison and the map, in `dictionary_ms`.
//!
//! ```text
//! cargo run --release -p columnar-engine --example dictionary_cost_probe -- \
//!     data/lineitem-rg6.parquet --out report.json
//! ```

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::{Array, ArrayRef, DictionaryArray, Float64Array, StringArray};
use arrow::datatypes::{DataType as ArrowType, Field, FieldRef, Int32Type, Schema};
use columnar_core::{ComparisonOp, ScalarValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::{ParquetSource, MAX_READER_BATCH_ROWS};
use columnar_kernels::Flavor;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder,
};
use parquet::arrow::ProjectionMask;

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;
/// The file's root index of `l_shipdate`.
const SHIPDATE: usize = 10;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The columns the probe reads, and the type of their dictionary's values.
///
/// The probe asks the crate for a dictionary for each of them, so the crate
/// keeps the indices and builds the dictionary once a row group.
const DICTIONARIES: [(usize, &str, ArrowType); 3] = [
    (QUANTITY, "l_quantity", ArrowType::Float64),
    (DISCOUNT, "l_discount", ArrowType::Float64),
    (SHIPDATE, "l_shipdate", ArrowType::Utf8),
];

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = Path::new(&path);
    let mut out_path: Option<String> = None;
    while let Some(argument) = arguments.next() {
        if argument == "--out" {
            out_path = arguments.next();
        }
    }

    let plan = q6_pushdown();
    let batch_rows = engine_batch_rows(path);
    let groups = row_groups(path);
    let projection = [QUANTITY, DISCOUNT, SHIPDATE];

    println!("file: {}", path.display());
    println!("row groups: {groups}, batch rows: {batch_rows}, iterations: {ITERATIONS}\n");

    // What the kept reader hands back, so the measurement is not taken on a
    // shape nobody can use.
    describe_kept(path, &projection, batch_rows);
    let (same_dictionary, batches_seen) =
        describe_dictionary_identity(path, &projection, batch_rows);

    // One column at a time, in both modes: a column that is cheap to keep and
    // a column that is not are different findings.
    let per_column = measure_columns(path, batch_rows);

    // The engine's comparison, one predicate at a time: the cost of a
    // predicate over the values, and the cost of the dictionary path's map.
    let per_predicate = measure_predicates(path, &projection, batch_rows);

    let engine_read = measure_engine_read(path, &plan);
    let expanded = repeat(|| crate_read(path, &projection, batch_rows, false).0);
    let kept = repeat(|| crate_read(path, &projection, batch_rows, true).0);
    let engine_expanded =
        repeat(|| crate_read_with_pushdown(path, &projection, batch_rows, &plan).0);
    let mut distinct_ms = 0.0;
    let mut map_ms = 0.0;
    let engine_kept = repeat(|| {
        let (elapsed, _, distinct, map) =
            crate_read_with_distinct_values(path, &projection, batch_rows);
        distinct_ms = distinct;
        map_ms = map;
        elapsed
    });

    println!("--- the engine's own read ---");
    println!("the engine's read: {:7.2} ms", ms(median(&engine_read)));
    println!();
    println!("--- the crate's read ---");
    println!("expanded         : {:7.2} ms", ms(median(&expanded)));
    println!("kept             : {:7.2} ms", ms(median(&kept)));
    println!("--- with the engine's comparison on top ---");
    println!("over the values  : {:7.2} ms", ms(median(&engine_expanded)));
    println!("over the distinct: {:7.2} ms", ms(median(&engine_kept)));
    println!();

    // The two paths must answer the same question. A probe that measures two
    // different answers measures nothing.
    let survivors_expanded = crate_read_with_pushdown(path, &projection, batch_rows, &plan).1;
    let survivors_kept = crate_read_with_distinct_values(path, &projection, batch_rows).1;
    if survivors_expanded == survivors_kept {
        println!("both paths keep {survivors_expanded} rows");
    } else {
        println!(
            "the two paths disagree: {survivors_expanded} rows against {survivors_kept}. \
             The measurement below is not comparable."
        );
    }
    println!();

    let mut report = serde_json::json!({
        "probe": "dictionary-cost",
        "file": path.display().to_string(),
        "row_groups": groups,
        "batch_rows": batch_rows,
        "iterations": ITERATIONS,
        "commit": commit(),
        "host_load_average": load_average(),
        "survivors": survivors_expanded,
        "dictionary_reuse": {
            "same_array_batches": same_dictionary,
            "batches": batches_seen,
        },
        "measurements_ms": {
            "crate_expanded": ms(median(&expanded)),
            "crate_kept": ms(median(&kept)),
            "engine_expanded": ms(median(&engine_expanded)),
            "engine_kept": ms(median(&engine_kept)),
            "engine_read": ms(median(&engine_read)),
        },
        "dictionary_ms": {
            "distinct": distinct_ms,
            "map": map_ms,
        },
    });
    if let Some(object) = report.as_object_mut() {
        object.insert("columns_ms".to_owned(), serde_json::Value::Object(per_column));
        object.insert("predicates_ms".to_owned(), serde_json::Value::Object(per_predicate));
    }

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = out_path {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }
}

/// The Q6 revenue query's predicate set, as the engine pushes it.
fn q6_pushdown() -> PushdownPlan {
    let mut plan = PushdownPlan::new();

    let mut shipdate = Pushdown::new(
        SHIPDATE,
        "l_shipdate",
        ComparisonOp::GtEq,
        ScalarValue::Utf8("1994-01-01".to_owned()),
    );
    shipdate.and(ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    plan.push(shipdate);

    let mut discount =
        Pushdown::new(DISCOUNT, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05));
    discount.and(ComparisonOp::LtEq, ScalarValue::Float64(0.07));
    plan.push(discount);

    plan.push(Pushdown::new(QUANTITY, "l_quantity", ComparisonOp::Lt, ScalarValue::Float64(24.0)));

    plan
}

/// Whether the crate hands the **same** dictionary to every batch of a row
/// group.
///
/// The engine could compare the distinct values once a row group instead of
/// once a batch, if it can tell that the dictionary has not changed. The
/// crate builds the dictionary once a row group, so the answer should be yes
/// -- and the probe checks it rather than assuming it.
fn describe_dictionary_identity(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
) -> (usize, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(batch_rows)
        .build()
        .expect("the reader builds");
    let mut previous: Option<ArrayRef> = None;
    let mut batches = 0usize;
    let mut same = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        let array = batch.column(2);
        let Some(dictionary) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>() else {
            println!("the shipdate is not a dictionary in the engine's own reader");
            return (0, 0);
        };
        let current = dictionary.values().clone();
        if let Some(last) = &previous {
            if Arc::ptr_eq(last, &current) {
                same += 1;
            }
        }
        previous = Some(current);
        batches += 1;
        if batches >= 24 {
            break;
        }
    }
    println!(
        "the dictionary is the same array in {same} of the first {} batches",
        batches.saturating_sub(1)
    );
    (same, batches.saturating_sub(1))
}

/// Prints what the kept reader hands back for each projected column.
fn describe_kept(path: &Path, columns: &[usize], batch_rows: usize) {
    let reader = dictionary_reader(path, columns, batch_rows);
    let Some(batch) = reader.into_iter().next() else {
        println!("the file has no rows");
        return;
    };
    {
        let batch = batch.expect("the batch");
        for (index, field) in batch.schema().fields().iter().enumerate() {
            let array = batch.column(index);
            let kind = match array.data_type() {
                ArrowType::Dictionary(_, value) => {
                    let dictionary = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>();
                    match dictionary {
                        Some(dictionary) => format!(
                            "dictionary over {}, {} values",
                            value,
                            dictionary.values().len()
                        ),
                        None => "dictionary, but not over int32 keys".to_owned(),
                    }
                }
                other => format!("{other}"),
            };
            println!("column {index}: {field} -- {kind}");
        }
    }
    println!();
}

/// The shipdate's predicate alone.
fn q6_pushdown_shipdate() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut shipdate = Pushdown::new(
        SHIPDATE,
        "l_shipdate",
        ComparisonOp::GtEq,
        ScalarValue::Utf8("1994-01-01".to_owned()),
    );
    shipdate.and(ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    plan.push(shipdate);
    plan
}

/// The discount's predicate alone.
fn q6_pushdown_discount() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount =
        Pushdown::new(DISCOUNT, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05));
    discount.and(ComparisonOp::LtEq, ScalarValue::Float64(0.07));
    plan.push(discount);
    plan
}

/// The quantity's predicate alone.
fn q6_pushdown_quantity() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    plan.push(Pushdown::new(QUANTITY, "l_quantity", ComparisonOp::Lt, ScalarValue::Float64(24.0)));
    plan
}

/// The crate's read of each column on its own, expanded and kept.
fn measure_columns(path: &Path, batch_rows: usize) -> serde_json::Map<String, serde_json::Value> {
    let mut per_column = serde_json::Map::new();
    for (column, name) in [(QUANTITY, "quantity"), (DISCOUNT, "discount"), (SHIPDATE, "shipdate")] {
        let alone = [column];
        let expanded = ms(median(&repeat(|| crate_read(path, &alone, batch_rows, false).0)));
        let kept = ms(median(&repeat(|| crate_read(path, &alone, batch_rows, true).0)));
        println!("column {column:>2}         : expanded {expanded:7.2} ms, kept {kept:7.2} ms");
        per_column.insert(format!("crate_expanded_{name}"), expanded.into());
        per_column.insert(format!("crate_kept_{name}"), kept.into());
    }
    println!();
    per_column
}

/// The engine's comparison, one predicate at a time.
fn measure_predicates(
    path: &Path,
    projection: &[usize],
    batch_rows: usize,
) -> serde_json::Map<String, serde_json::Value> {
    let mut per_predicate = serde_json::Map::new();
    for (name, key, plan) in [
        ("the shipdate alone", "shipdate", q6_pushdown_shipdate()),
        ("the discount alone", "discount", q6_pushdown_discount()),
        ("the quantity alone", "quantity", q6_pushdown_quantity()),
    ] {
        let alone =
            ms(median(&repeat(|| crate_read_with_pushdown(path, projection, batch_rows, &plan).0)));
        println!("{name:<20}: {alone:7.2} ms");
        per_predicate.insert(format!("pushdown_{key}"), alone.into());
    }
    println!();
    per_predicate
}

/// The engine's own read, with nothing materialised.
fn measure_engine_read(path: &Path, plan: &PushdownPlan) -> Vec<Duration> {
    repeat(|| {
        let source = ParquetSource::open(path).expect("the source opens");
        let started = Instant::now();
        let dataset = source.read(&[], plan, columnar_kernels::best(), 1).expect("the read runs");
        let elapsed = started.elapsed();
        std::hint::black_box(&dataset);
        elapsed
    })
}

/// The crate's read of some columns, with the dictionaries expanded or kept.
///
/// The crate's reader is given an Arrow schema: the file's own, or one that
/// declares the dictionary-encoded columns as dictionaries. The second is what
/// the engine would need to ask for to keep the indices.
fn crate_read(path: &Path, columns: &[usize], batch_rows: usize, kept: bool) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let reader = if kept {
        dictionary_reader(path, columns, batch_rows)
    } else {
        builder
            .with_projection(mask)
            .with_batch_size(batch_rows)
            .build()
            .expect("the reader builds")
    };

    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        rows += batch.num_rows();
    }
    (started.elapsed(), rows)
}

/// The crate's reader, with the dictionary-encoded columns kept as
/// dictionaries.
fn dictionary_reader(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
) -> parquet::arrow::arrow_reader::ParquetRecordBatchReader {
    let file = File::open(path).expect("the file opens");
    let plain = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let schema = plain.schema().clone();
    let options = ArrowReaderOptions::new().with_schema(Arc::new(dictionary_schema(&schema)));
    let file = File::open(path).expect("the file opens");
    let metadata = ArrowReaderMetadata::load(&file, options).expect("the metadata loads");
    let builder = ParquetRecordBatchReaderBuilder::new_with_metadata(file, metadata);
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    builder.with_projection(mask).with_batch_size(batch_rows).build().expect("the reader builds")
}

/// The file's Arrow schema, with the dictionary-encoded columns declared as
/// dictionaries.
fn dictionary_schema(schema: &Schema) -> Schema {
    let fields: Vec<FieldRef> = schema
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let dictionary = DICTIONARIES.iter().find(|(column, _, _)| *column == index);
            match dictionary {
                Some((_, _, value)) => Arc::new(Field::new(
                    field.name(),
                    ArrowType::Dictionary(Box::new(ArrowType::Int32), Box::new(value.clone())),
                    field.is_nullable(),
                )),
                None => field.clone(),
            }
        })
        .collect();
    Schema::new(fields)
}

/// The crate's read, with the engine's own pushdown on top of it.
///
/// This is today's path: the crate's values, and the engine's comparison over
/// every one of them.
fn crate_read_with_pushdown(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
    plan: &PushdownPlan,
) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(batch_rows)
        .build()
        .expect("the reader builds");
    let flavor = columnar_kernels::best();
    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        let mask = plan
            .evaluate_batch(&batch, flavor)
            .expect("the pushdown runs")
            .expect("the batch has a mask");
        rows += mask.count();
    }
    (started.elapsed(), rows)
}

/// The crate's read, with the dictionary kept, and the engine's comparison
/// over the **distinct values**.
///
/// This is the proposed path: the crate keeps the indices, the engine compares
/// each distinct value once, and the answer is mapped back through the
/// indices. It is the engine's own dictionary path, run over the numeric
/// dictionaries the engine never sees today.
fn crate_read_with_distinct_values(
    path: &Path,
    columns: &[usize],
    batch_rows: usize,
) -> (Duration, usize, f64, f64) {
    let reader = dictionary_reader(path, columns, batch_rows);
    let flavor = columnar_kernels::best();
    let started = Instant::now();
    let mut rows = 0usize;
    let mut distinct = Duration::ZERO;
    let mut map = Duration::ZERO;
    for batch in reader {
        let batch = batch.expect("the batch");
        let mut mask = Mask::new(batch.num_rows());
        // The batch holds the projected columns, in the file's order: the
        // quantity, the discount, the shipdate.
        for (column, comparisons) in [
            (0usize, vec![(ComparisonOp::Lt, ScalarValue::Float64(24.0))]),
            (
                1usize,
                vec![
                    (ComparisonOp::GtEq, ScalarValue::Float64(0.05)),
                    (ComparisonOp::LtEq, ScalarValue::Float64(0.07)),
                ],
            ),
            (
                2usize,
                vec![
                    (ComparisonOp::GtEq, ScalarValue::Utf8("1994-01-01".to_owned())),
                    (ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned())),
                ],
            ),
        ] {
            let array = batch.column(column);
            let Some(dictionary) = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>()
            else {
                continue;
            };
            let started = Instant::now();
            let distinct_values = distinct_matches(dictionary, &comparisons, flavor);
            distinct += started.elapsed();
            let started = Instant::now();
            let mapped = map_indices(dictionary, &distinct_values);
            map += started.elapsed();
            mask.and_with(&mapped);
        }
        rows += mask.count();
    }
    println!(
        "  the distinct values took {distinct_ms:.2} ms, the map took {map_ms:.2} ms",
        distinct_ms = distinct.as_secs_f64() * 1000.0,
        map_ms = map.as_secs_f64() * 1000.0,
    );
    (started.elapsed(), rows, distinct.as_secs_f64() * 1000.0, map.as_secs_f64() * 1000.0)
}

/// The match of every distinct value of a dictionary, in dictionary order.
///
/// The engine's rule: a NULL never matches, and the comparison is the
/// engine's own kernel over the values.
fn distinct_matches(
    dictionary: &DictionaryArray<Int32Type>,
    comparisons: &[(ComparisonOp, ScalarValue)],
    flavor: Flavor,
) -> Vec<bool> {
    let values = dictionary.values();
    let mut matched = vec![false; values.len()];
    if let Some(array) = values.as_any().downcast_ref::<Float64Array>() {
        let mut words = vec![0u64; array.len().div_ceil(64)];
        let mut first = true;
        for (op, bound) in comparisons {
            let ScalarValue::Float64(bound) = bound else {
                continue;
            };
            words.fill(0);
            columnar_kernels::compare_f64_into(
                flavor,
                array.values(),
                *op,
                *bound,
                None,
                &mut words,
            );
            for (index, bit) in matched.iter_mut().enumerate() {
                let hit = word_bit(&words, index);
                *bit = if first { hit } else { *bit && hit };
            }
            first = false;
        }
        return matched;
    }
    if let Some(array) = values.as_any().downcast_ref::<StringArray>() {
        for (index, value) in array.iter().enumerate() {
            let Some(value) = value else {
                matched[index] = false;
                continue;
            };
            matched[index] = comparisons.iter().all(|(op, bound)| {
                let ScalarValue::Utf8(bound) = bound else {
                    return false;
                };
                compare_str(value, *op, bound)
            });
        }
        return matched;
    }
    matched
}

/// The bit at `index` of a bitmap.
fn word_bit(words: &[u64], index: usize) -> bool {
    words.get(index / 64).is_some_and(|word| word & (1u64 << (index % 64)) != 0)
}

/// Maps a dictionary's answer back through its indices.
///
/// The loop is branchless and accumulates a word at a time, exactly as the
/// engine's own dictionary path does: a conditional bit set is a mispredicted
/// branch every time a run ends.
fn map_indices(dictionary: &DictionaryArray<Int32Type>, distinct: &[bool]) -> Vec<u64> {
    let keys = dictionary.keys();
    let indices: &[i32] = keys.values();
    let rows = indices.len();
    let mut mask = vec![0u64; rows.div_ceil(64)];
    for (chunk_index, chunk) in indices.chunks(64).enumerate() {
        let mut word = 0u64;
        for (bit, &key) in chunk.iter().enumerate() {
            let entry = usize::try_from(key).unwrap_or(usize::MAX);
            let matched = u64::from(distinct.get(entry).copied().unwrap_or(false));
            word |= matched << bit;
        }
        if let Some(slot) = mask.get_mut(chunk_index) {
            *slot = word;
        }
    }
    mask
}

/// Compares two strings, byte-wise, as `DuckDB`'s binary collation does.
fn compare_str(value: &str, op: ComparisonOp, threshold: &str) -> bool {
    match op {
        ComparisonOp::Lt => value < threshold,
        ComparisonOp::LtEq => value <= threshold,
        ComparisonOp::Gt => value > threshold,
        ComparisonOp::GtEq => value >= threshold,
        ComparisonOp::Eq => value == threshold,
        ComparisonOp::Ne => value != threshold,
    }
}

/// A row mask: one bit a row, and the count of the rows that are set.
struct Mask {
    words: Vec<u64>,
    len: usize,
}

impl Mask {
    /// A mask of `len` rows, with every row set.
    ///
    /// The mask is the **and** of the predicates' masks, so it starts as all
    /// ones and each predicate narrows it.
    fn new(len: usize) -> Self {
        let count = len.div_ceil(64);
        let mut words = vec![u64::MAX; count];
        let last = count.saturating_sub(1) * 64;
        if let Some(word) = words.get_mut(count.saturating_sub(1)) {
            *word = mask_bits(len.saturating_sub(last));
        }
        Self { words, len }
    }

    /// Ands another mask into this one.
    fn and_with(&mut self, other: &[u64]) {
        for (index, word) in self.words.iter_mut().enumerate() {
            *word &= other.get(index).copied().unwrap_or(0);
        }
    }

    /// How many rows the mask keeps.
    fn count(&self) -> usize {
        let mut count = 0usize;
        for (index, word) in self.words.iter().enumerate() {
            let bits = self.len.saturating_sub(index * 64).min(64);
            count += count_ones(word & mask_bits(bits));
        }
        count
    }
}

/// The low `bits` bits of a word.
fn mask_bits(bits: usize) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

/// How many bits are set in a word.
fn count_ones(word: u64) -> usize {
    word.count_ones() as usize
}

/// The number of times a measurement is repeated, and the median.
fn repeat<T>(mut measurement: impl FnMut() -> T) -> Vec<T> {
    (0..ITERATIONS).map(|_| measurement()).collect()
}

/// The median of a set of durations.
fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The file's row-group count.
fn row_groups(path: &Path) -> usize {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    builder.metadata().num_row_groups()
}

/// The reader's batch size the engine would use for this file.
///
/// The probe **asks the engine** for it, rather than copying the rule out of
/// it: the engine derives the batch size from the file -- the batch size
/// divides every row group's row count, so no row group is left with a
/// partial batch -- and the probe's shapes have to be the engine's shapes.
fn engine_batch_rows(path: &Path) -> usize {
    let source = ParquetSource::open(path).expect("the source opens");
    source.batch_rows_under(MAX_READER_BATCH_ROWS)
}

/// The commit the working tree is at, for the report.
fn commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |text| text.trim().to_owned())
}

/// The host's load average, for the report.
fn load_average() -> f64 {
    std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| {
            text.trim()
                .trim_start_matches('{')
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<f64>().ok())
        })
        .unwrap_or(0.0)
}
