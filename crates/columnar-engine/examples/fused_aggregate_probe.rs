//! The fused aggregate: what the projection costs, and what a fused fold
//! would cost instead.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind [ADR-0016](../../../docs/adr/0016-the-aggregate-reads-in-place.md).
//! It answers one question: **what does the projection's materialisation cost
//! `filter-range`, and what would the aggregate cost if it read the source
//! column in place?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example fused_aggregate_probe -- data/lineitem.parquet
//! ```
//!
//! The query is the engine's own `filter-range`: `sum(l_quantity)` over the
//! rows whose `l_discount` is in `[0.04, 0.07]`. The selection is the real
//! one -- the engine's filter semantics, over the engine's own kernel -- and
//! every candidate folds the **same** selection, in the **same** order, so the
//! answers are comparable bit for bit.
//!
//! Four candidates are measured:
//!
//! 1. **gather, then fold** -- what the engine does today. The projection
//!    materialises the survivors into a new column, and the aggregate reads
//!    that column.
//! 2. **scattered fold** -- the aggregate reads the source column at the
//!    selection's rows. No intermediate column is ever built.
//! 3. **sequential fold** -- the aggregate walks the source column in row
//!    order and asks the selection whether each row survived.
//! 4. **the whole column** -- the sum of every row, with no selection at all.
//!    This is the floor: no fold can be cheaper than reading the column.

// The sample index below is a literal into a vector the function has just
// filled, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]

use std::path::Path;
use std::time::{Duration, Instant};

use columnar_core::{BatchData, ColumnChunk, ComparisonOp, Encoding, NullBitmap, SelectionVector};
use columnar_engine::operators::project;
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::ParquetSource;

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 7;

/// The median of a set of durations.
fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The dense values of a float column chunk.
///
/// A dictionary-encoded column is expanded, exactly as the core's decoder
/// expands it, so the measurement is over the values the aggregate would read
/// either way.
fn dense_values(chunk: &ColumnChunk) -> Vec<f64> {
    match &chunk.encoding {
        Encoding::Float64 { values } => values.clone(),
        Encoding::Float64Dict { dict, indices } => indices
            .iter()
            .map(|&index| {
                let index = usize::try_from(index).expect("a dictionary index is not negative");
                *dict.get(index).expect("a dictionary index is in range")
            })
            .collect(),
        other => panic!("the probe expects a float column, found {}", other.name()),
    }
}

/// The filter's selection, built with the engine's own kernel.
///
/// The two predicates are on the same column, so the engine's filter evaluates
/// them independently and ANDs the masks. This is the same thing.
fn discount_selection(discount: &[f64], rows: usize) -> (Vec<u64>, SelectionVector) {
    let flavor = columnar_kernels::best();
    let mut lower = vec![0u64; rows.div_ceil(64)];
    let mut upper = vec![0u64; rows.div_ceil(64)];
    columnar_kernels::compare_f64_into(
        flavor,
        discount,
        ComparisonOp::GtEq,
        0.04,
        None,
        &mut lower,
    );
    columnar_kernels::compare_f64_into(
        flavor,
        discount,
        ComparisonOp::LtEq,
        0.07,
        None,
        &mut upper,
    );
    let words: Vec<u64> = lower.iter().zip(upper.iter()).map(|(a, b)| a & b).collect();
    let count = words.iter().map(|word| word.count_ones() as usize).sum::<usize>();
    let selection = SelectionVector::from_filter(&words, count, rows);
    (words, selection)
}

/// The engine's current path: gather the survivors, then fold the gathered
/// column.
fn gather_then_fold(chunk: &ColumnChunk, selection: &SelectionVector) -> f64 {
    let gathered = project::gather(chunk, selection);
    let BatchData::Float64(values) = &gathered.data else {
        panic!("the gathered column is a float column")
    };
    let mut total = 0.0f64;
    for (row, value) in values.iter().enumerate() {
        if gathered.validity.is_valid(row) {
            total += value;
        }
    }
    total
}

/// The fused path: read the source column at the selection's rows.
fn scattered_fold(values: &[f64], validity: &NullBitmap, selection: &SelectionVector) -> f64 {
    let mut total = 0.0f64;
    selection.for_each(|row| {
        let row = usize::try_from(row).expect("a row fits in a usize");
        if validity.is_valid(row) {
            total += values[row];
        }
    });
    total
}

/// The fused path, the other way: walk the source column and ask the
/// selection about each row.
fn sequential_fold(values: &[f64], validity: &NullBitmap, selection: &SelectionVector) -> f64 {
    let mut total = 0.0f64;
    for (row, value) in values.iter().enumerate() {
        if selection.contains(u32::try_from(row).unwrap_or(u32::MAX)) && validity.is_valid(row) {
            total += value;
        }
    }
    total
}

/// The fused path, word by word: a word that is empty is skipped whole, and a
/// word that is full is added whole.
fn word_wise_fold(values: &[f64], validity: &NullBitmap, words: &[u64], rows: usize) -> f64 {
    let mut total = 0.0f64;
    for (word_index, &word) in words.iter().enumerate() {
        let base = word_index * 64;
        if word == 0 {
            continue;
        }
        if word == u64::MAX {
            for (offset, value) in values[base..(base + 64).min(rows)].iter().enumerate() {
                let row = base + offset;
                if validity.is_valid(row) {
                    total += value;
                }
            }
            continue;
        }
        let mut bits = word;
        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            let row = base + bit;
            if row >= rows {
                break;
            }
            if validity.is_valid(row) {
                total += values[row];
            }
            bits &= bits - 1;
        }
    }
    total
}

/// The floor: the sum of the whole column, with no selection at all.
fn whole_column(values: &[f64], validity: &NullBitmap) -> f64 {
    let mut total = 0.0f64;
    for (row, value) in values.iter().enumerate() {
        if validity.is_valid(row) {
            total += value;
        }
    }
    total
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = Path::new(&path);

    let source = ParquetSource::open(path).expect("the source opens");
    let flavor = columnar_kernels::best();
    let dataset =
        source.read(&[DISCOUNT, QUANTITY], &PushdownPlan::new(), flavor, 1).expect("the read runs");
    let rows = dataset.len();

    let discount_index = dataset
        .chunks
        .iter()
        .position(|chunk| chunk.name == "l_discount")
        .expect("the dataset holds l_discount");
    let quantity_index = dataset
        .chunks
        .iter()
        .position(|chunk| chunk.name == "l_quantity")
        .expect("the dataset holds l_quantity");

    let discount = dense_values(&dataset.chunks[discount_index]);
    let quantity_chunk = &dataset.chunks[quantity_index];
    let quantity = dense_values(quantity_chunk);
    let validity = &quantity_chunk.validity;

    let (words, selection) = discount_selection(&discount, rows);
    let survivors = selection.len();

    println!("file: {}", path.display());
    println!("rows: {rows}, survivors: {survivors} ({:.1}% density)", selection.density() * 100.0);
    println!("selection: {:?}\n", selection.kind());

    // --- The answers -------------------------------------------------------
    let baseline = gather_then_fold(quantity_chunk, &selection);
    let scattered = scattered_fold(&quantity, validity, &selection);
    let sequential = sequential_fold(&quantity, validity, &selection);
    let word_wise = word_wise_fold(&quantity, validity, &words, rows);
    let whole = whole_column(&quantity, validity);

    println!("--- the answers ---");
    println!("gather then fold : {baseline:.9}  (bits {:016x})", baseline.to_bits());
    println!("scattered fold   : {scattered:.9}  (bits {:016x})", scattered.to_bits());
    println!("sequential fold  : {sequential:.9}  (bits {:016x})", sequential.to_bits());
    println!("word-wise fold   : {word_wise:.9}  (bits {:016x})", word_wise.to_bits());
    println!("whole column     : {whole:.9}  (bits {:016x})", whole.to_bits());
    let identical = baseline.to_bits() == scattered.to_bits()
        && baseline.to_bits() == sequential.to_bits()
        && baseline.to_bits() == word_wise.to_bits();
    println!("all four folds agree: {identical}\n");

    // --- The costs ---------------------------------------------------------
    let mut gather_samples = Vec::with_capacity(ITERATIONS);
    let mut scattered_samples = Vec::with_capacity(ITERATIONS);
    let mut sequential_samples = Vec::with_capacity(ITERATIONS);
    let mut word_wise_samples = Vec::with_capacity(ITERATIONS);
    let mut whole_samples = Vec::with_capacity(ITERATIONS);

    // The candidates are interleaved, so no candidate gets the cache the
    // previous one left behind.
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        std::hint::black_box(gather_then_fold(quantity_chunk, &selection));
        gather_samples.push(started.elapsed());

        let started = Instant::now();
        std::hint::black_box(scattered_fold(&quantity, validity, &selection));
        scattered_samples.push(started.elapsed());

        let started = Instant::now();
        std::hint::black_box(sequential_fold(&quantity, validity, &selection));
        sequential_samples.push(started.elapsed());

        let started = Instant::now();
        std::hint::black_box(word_wise_fold(&quantity, validity, &words, rows));
        word_wise_samples.push(started.elapsed());

        let started = Instant::now();
        std::hint::black_box(whole_column(&quantity, validity));
        whole_samples.push(started.elapsed());
    }

    let gather_ms = ms(median(&mut gather_samples));
    let scattered_ms = ms(median(&mut scattered_samples));
    let sequential_ms = ms(median(&mut sequential_samples));
    let word_wise_ms = ms(median(&mut word_wise_samples));
    let whole_ms = ms(median(&mut whole_samples));

    println!("--- the costs, median of {ITERATIONS} ---");
    println!("gather then fold : {gather_ms:7.2} ms");
    println!("scattered fold   : {scattered_ms:7.2} ms");
    println!("sequential fold  : {sequential_ms:7.2} ms");
    println!("word-wise fold   : {word_wise_ms:7.2} ms");
    println!("whole column     : {whole_ms:7.2} ms");
    println!();
    println!("scattered saves  : {:7.2} ms", gather_ms - scattered_ms);
    println!("sequential saves: {:7.2} ms", gather_ms - sequential_ms);
    println!("word-wise saves  : {:7.2} ms", gather_ms - word_wise_ms);
}
