//! `filter-range`'s lazy scan, decomposed.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind [ADR-0017](../../../docs/adr/0017-the-scan-filters-only-what-it-keeps.md).
//! It answers one question: **where does the lazy scan of `filter-range` go?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example filter_range_scan_decomposition -- data/lineitem.parquet
//! ```
//!
//! `filter-range` is `sum(l_quantity) WHERE l_discount >= 0.04 AND l_discount <= 0.07`.
//! The lazy path reads **two** columns: `l_discount`, because the predicates
//! need it, and `l_quantity`, because the aggregate sums it. It then filters
//! the batch down to the survivors -- and the measurement is about what that
//! filter touches, and what happens to the values afterwards.
//!
//! Four things are measured, on the same file, in the same process, warm:
//!
//! 1. **Arrow's own decode** of the two columns, with no filter and no
//!    conversion -- what the reader costs on its own.
//! 2. **The predicates**, over the whole column.
//! 3. **The filter**, as the engine runs it: the whole batch is filtered, so
//!    the pushed-down column is compacted too -- and it is then thrown away.
//! 4. **The copy** of the survivors into the engine's own chunk, which the
//!    builder does after the filter has already made a copy of them.
//!
//! And three ways of doing the same work, so the cost of each shape is a
//! measurement rather than an argument:
//!
//! - **A — the engine's shape today**: filter the whole batch, then copy the
//!   materialised column out of it.
//! - **B — filter only what is kept**: the pushed-down column is not
//!   materialised, so compacting it is work nobody asked for.
//! - **C — compact once**: the engine's own bit loop writes the survivors
//!   straight into the chunk, so the values are copied once instead of twice.

// The sample index below is a literal into a vector the function has just
// filled, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::array::{Float64Array, RecordBatch};
use arrow::compute::filter_record_batch;
use arrow::datatypes::{DataType, Field, Schema};
use columnar_core::ComparisonOp;
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::ParquetSource;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;

/// The batch size the lazy path uses.
const READER_BATCH_ROWS: usize = 8_192;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The reader, over the two columns the query needs.
fn reader(
    path: &Path,
    columns: &[usize],
) -> impl Iterator<Item = arrow::error::Result<RecordBatch>> {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    builder
        .with_projection(mask)
        .with_batch_size(READER_BATCH_ROWS)
        .build()
        .expect("the reader builds")
}

/// The predicates' mask over a batch, in the engine's own layout.
///
/// The engine's pushdown makes **one pass per comparison** over the column's
/// values and ANDs the masks afterwards. This is that, measured.
fn predicate_mask(array: &Float64Array) -> (Duration, Vec<u64>) {
    let flavor = columnar_kernels::best();
    let rows = array.len();
    let mut lower = vec![0u64; rows.div_ceil(64)];
    let mut upper = vec![0u64; rows.div_ceil(64)];
    let started = Instant::now();
    columnar_kernels::compare_f64_into(
        flavor,
        array.values(),
        ComparisonOp::GtEq,
        0.04,
        None,
        &mut lower,
    );
    columnar_kernels::compare_f64_into(
        flavor,
        array.values(),
        ComparisonOp::LtEq,
        0.07,
        None,
        &mut upper,
    );
    let elapsed = started.elapsed();
    let words: Vec<u64> = lower.iter().zip(upper.iter()).map(|(a, b)| a & b).collect();
    (elapsed, words)
}

/// The mask as an Arrow boolean array, the way the engine hands it over.
fn as_array(words: &[u64], rows: usize) -> arrow::array::BooleanArray {
    arrow::array::BooleanArray::new(
        arrow::buffer::BooleanBuffer::new(arrow::buffer::Buffer::from_vec(words.to_vec()), 0, rows),
        None,
    )
}

/// The engine's shape today: filter the whole batch, then copy the
/// materialised column out of the filtered batch.
fn shape_a(path: &Path) -> (Duration, usize) {
    let mut elapsed = Duration::ZERO;
    let mut rows = 0usize;
    for batch in reader(path, &[QUANTITY, DISCOUNT]) {
        let batch = batch.expect("the batch");
        let discount = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_discount is a float column");
        let (_, words) = predicate_mask(discount);
        let started = Instant::now();
        let filtered = filter_record_batch(&batch, &as_array(&words, batch.num_rows()))
            .expect("the filter runs");
        elapsed += started.elapsed();
        let quantity = filtered
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_quantity is a float column");
        let started = Instant::now();
        let mut chunk: Vec<f64> = Vec::with_capacity(quantity.len());
        chunk.extend_from_slice(quantity.values());
        elapsed += started.elapsed();
        std::hint::black_box(&chunk);
        rows += filtered.num_rows();
    }
    (elapsed, rows)
}

/// Filter only what is kept: the pushed-down column is not materialised, so
/// compacting it is work nobody asked for.
fn shape_b(path: &Path) -> (Duration, usize) {
    let schema = Arc::new(Schema::new(vec![Field::new("l_quantity", DataType::Float64, true)]));
    let mut elapsed = Duration::ZERO;
    let mut rows = 0usize;
    for batch in reader(path, &[QUANTITY, DISCOUNT]) {
        let batch = batch.expect("the batch");
        let discount = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_discount is a float column");
        let (_, words) = predicate_mask(discount);
        let started = Instant::now();
        let only = batch.slice(0, batch.num_rows());
        let one = RecordBatch::try_new(schema.clone(), vec![only.column(0).clone()])
            .expect("the one-column batch");
        let filtered = filter_record_batch(&one, &as_array(&words, batch.num_rows()))
            .expect("the filter runs");
        elapsed += started.elapsed();
        let quantity = filtered
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_quantity is a float column");
        let started = Instant::now();
        let mut chunk: Vec<f64> = Vec::with_capacity(quantity.len());
        chunk.extend_from_slice(quantity.values());
        elapsed += started.elapsed();
        std::hint::black_box(&chunk);
        rows += filtered.num_rows();
    }
    (elapsed, rows)
}

/// Compact once: the engine's own bit loop writes the survivors straight into
/// the chunk, so the values are copied once instead of twice.
fn shape_c(path: &Path) -> (Duration, usize) {
    let mut elapsed = Duration::ZERO;
    let mut rows = 0usize;
    for batch in reader(path, &[QUANTITY, DISCOUNT]) {
        let batch = batch.expect("the batch");
        let discount = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_discount is a float column");
        let (_, words) = predicate_mask(discount);
        let quantity = batch
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_quantity is a float column");
        let started = Instant::now();
        let mut chunk: Vec<f64> = Vec::with_capacity(quantity.len());
        for (word_index, &word) in words.iter().enumerate() {
            let base = word_index * 64;
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let row = base + bit;
                if row < quantity.len() {
                    chunk.push(quantity.values()[row]);
                }
                bits &= bits - 1;
            }
        }
        elapsed += started.elapsed();
        std::hint::black_box(&chunk);
        rows += chunk.len();
    }
    (elapsed, rows)
}

/// Arrow's own decode of the two columns, with no filter and no conversion.
fn decode_only(path: &Path) -> (Duration, usize) {
    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader(path, &[QUANTITY, DISCOUNT]) {
        let batch = batch.expect("the batch");
        rows += batch.num_rows();
    }
    (started.elapsed(), rows)
}

/// The engine's own scan, end to end.
///
/// The predicates are the **same conjunction** the shapes above use, so the
/// scan keeps the same rows they keep -- and a reader can check that.
fn engine_scan(path: &Path) -> (Duration, (usize, u64)) {
    let mut plan = PushdownPlan::new();
    let mut discount = columnar_engine::pushdown::Pushdown::new(
        DISCOUNT,
        "l_discount",
        ComparisonOp::GtEq,
        columnar_core::ScalarValue::Float64(0.04),
    );
    discount.and(ComparisonOp::LtEq, columnar_core::ScalarValue::Float64(0.07));
    plan.push(discount);
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset =
        source.read(&[QUANTITY], &plan, columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    let predicates_ns = plan.metrics().predicate_ns;
    std::hint::black_box(&dataset);
    (elapsed, (rows, predicates_ns))
}

/// Repeats a measurement and returns the median, with whatever it measured.
fn repeat<T>(mut measure: impl FnMut() -> (Duration, T)) -> (Duration, T) {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        samples.push(measure());
    }
    let mut order: Vec<usize> = (0..samples.len()).collect();
    order.sort_by_key(|&index| samples[index].0);
    let middle = order[order.len() / 2];
    samples.swap_remove(middle)
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = Path::new(&path);

    println!("file: {}", path.display());
    println!("iterations: {ITERATIONS}, batch rows: {READER_BATCH_ROWS}\n");

    // --- Arrow's own decode ------------------------------------------------
    let (decode, decoded_rows) = repeat(|| decode_only(path));
    let decode_ms = ms(decode);
    println!("--- Arrow's own decode, no filter, no conversion ---");
    println!("l_quantity + l_discount : {decode_ms:7.1} ms   ({decoded_rows} rows)");
    println!();

    // --- The three shapes --------------------------------------------------
    let (a, a_rows) = repeat(|| shape_a(path));
    let (b, b_rows) = repeat(|| shape_b(path));
    let (c, c_rows) = repeat(|| shape_c(path));
    println!("--- the filter and the copy, in three shapes ---");
    println!("A  filter the batch, then copy : {:7.1} ms   ({a_rows} rows)", ms(a));
    println!("B  filter only what is kept     : {:7.1} ms   ({b_rows} rows)", ms(b));
    println!("C  compact once, into the chunk : {:7.1} ms   ({c_rows} rows)", ms(c));
    println!();

    // --- The engine's own scan ---------------------------------------------
    let (scan, (rows, predicates_ns)) = repeat(|| engine_scan(path));
    let predicates_ms = Duration::from_nanos(predicates_ns).as_secs_f64() * 1000.0;
    println!("--- the engine's own scan ---");
    println!("the whole scan        : {:7.1} ms   ({rows} rows survive)", ms(scan));
    println!("  of which predicates : {predicates_ms:7.1} ms");
    println!("  of which the decode : {decode_ms:7.1} ms");
    println!("  the rest is the filter and the copy");
}
