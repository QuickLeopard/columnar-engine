//! The lazy scan's decomposition.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind [ADR-0015](../../../docs/adr/0015-the-engine-keeps-the-mask.md).
//! It answers one question: **where does the lazy scan of Q6's two columns
//! go?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example lazy_scan_decomposition -- data/lineitem.parquet
//! ```
//!
//! It measures four things, all on the same file, in the same process, warm:
//!
//! 1. **Arrow's own decode** of the two columns, with no filter and no
//!    engine conversion — what the reader costs on its own.
//! 2. **The mask**, and what it costs to turn it into the row-range list that
//!    a Parquet row filter is: parquet-rs applies such a list one range at a
//!    time, through one `read_records` call per range.
//! 3. **Reading a column through that list**, against reading it without
//!    one. This is the defect: the list is longer than the number of rows it
//!    selects, and it costs several times the decode.
//! 4. **The engine's own scan**, which keeps the mask and applies it in the
//!    engine's domain, together with the cost of the predicates themselves.

// The sample index below is a literal into a vector the function has just
// filled, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

use arrow::array::{BooleanArray, Float64Array};
use arrow::buffer::{BooleanBuffer, Buffer};
use columnar_core::{ComparisonOp, ScalarValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::ParquetSource;
use columnar_kernels::Flavor;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReaderBuilder, RowSelection};
use parquet::arrow::ProjectionMask;

/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;
/// The file's root index of `l_shipdate`.
const SHIPDATE: usize = 10;

/// The batch size the lazy path uses.
const READER_BATCH_ROWS: usize = 8_192;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The median of a set of durations.
fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Q6's predicate set, as the engine pushes it.
fn q6_pushdown() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount =
        Pushdown::new(DISCOUNT, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04));
    discount.and(ComparisonOp::LtEq, ScalarValue::Float64(0.07));
    plan.push(discount);

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

/// Arrow's own decode of some columns: the reader, with a projection mask and
/// **no** row filter. The batches are counted and dropped.
fn decode_only(path: &Path, columns: &[usize]) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READER_BATCH_ROWS)
        .build()
        .expect("the reader builds");
    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        rows += batch.num_rows();
    }
    (started.elapsed(), rows)
}

/// The engine's own kernel over `l_discount`, and the row selection its mask
/// becomes.
///
/// This is what a Parquet row filter would do with the mask: turn it into a
/// list of row ranges.
fn discount_mask(path: &Path) -> (Duration, usize, usize, RowSelection) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), [DISCOUNT]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READER_BATCH_ROWS)
        .build()
        .expect("the reader builds");

    let flavor = columnar_kernels::best();
    let mut masks: Vec<BooleanArray> = Vec::new();
    for batch in reader {
        let batch = batch.expect("the batch");
        let array = batch
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("the column is a float column");
        let rows = array.len();
        let mut words = vec![0u64; rows.div_ceil(64)];
        let mut upper = vec![0u64; rows.div_ceil(64)];
        columnar_kernels::compare_f64_into(
            flavor,
            array.values(),
            ComparisonOp::GtEq,
            0.04,
            None,
            &mut words,
        );
        columnar_kernels::compare_f64_into(
            flavor,
            array.values(),
            ComparisonOp::LtEq,
            0.07,
            None,
            &mut upper,
        );
        for (kept, computed) in words.iter_mut().zip(upper.iter()) {
            *kept &= computed;
        }
        masks.push(BooleanArray::new(BooleanBuffer::new(Buffer::from_vec(words), 0, rows), None));
    }

    let started = Instant::now();
    let selection = RowSelection::from_filters(&masks);
    let elapsed = started.elapsed();
    let selectors = selection.row_count();
    let ranges = selection.iter().count();
    (elapsed, selectors, ranges, selection)
}

/// The cost of reading a column with a row selection already applied.
///
/// This is what the *second* predicate's reader did: parquet-rs gave it the
/// first predicate's selection, and the selection's fragmentation is what it
/// had to walk -- one `read_records` call per range.
fn selection_read_cost(path: &Path, column: usize, selection: &RowSelection) -> (f64, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), [column]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READER_BATCH_ROWS)
        .with_row_selection(selection.clone())
        .build()
        .expect("the reader builds");
    let started = Instant::now();
    let mut rows = 0usize;
    for batch in reader {
        let batch = batch.expect("the batch");
        rows += batch.num_rows();
    }
    (ms(started.elapsed()), rows)
}

/// The engine's own lazy read, end to end.
fn engine_read(path: &Path, plan: &PushdownPlan) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source.read(&[], plan, columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// Repeats a measurement and returns the median.
fn repeat(mut measure: impl FnMut() -> Duration) -> Duration {
    let mut samples = Vec::with_capacity(ITERATIONS);
    // A warm-up: the first read pays for the page cache.
    samples.push(measure());
    for _ in 1..ITERATIONS {
        samples.push(measure());
    }
    median(&mut samples)
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = Path::new(&path);
    let plan = q6_pushdown();

    println!("file: {}", path.display());
    println!("iterations: {ITERATIONS}, batch rows: {READER_BATCH_ROWS}\n");

    // --- Arrow's own decode ------------------------------------------------
    let discount_ms = ms(repeat(|| decode_only(path, &[DISCOUNT]).0));
    let shipdate_ms = ms(repeat(|| decode_only(path, &[SHIPDATE]).0));
    let both_ms = ms(repeat(|| decode_only(path, &[DISCOUNT, SHIPDATE]).0));
    println!("--- Arrow's own decode, no filter, no conversion ---");
    println!("l_discount only   : {discount_ms:7.1} ms");
    println!("l_shipdate only   : {shipdate_ms:7.1} ms");
    println!("both columns      : {both_ms:7.1} ms");
    println!();

    // --- The mask, as a row-range list --------------------------------------
    let (from_filters, selectors, ranges, selection) = discount_mask(path);
    println!("--- the pushdown's mask, as a row-range list ---");
    println!("rows selected             : {selectors}");
    println!("ranges in the list        : {ranges}");
    println!("RowSelection::from_filters: {:.1} ms", ms(from_filters));
    println!();

    // --- Reading through the list, against reading without it --------------
    let (selected_ms, selected_rows) = selection_read_cost(path, SHIPDATE, &selection);
    println!("--- reading a column, with and without the list ---");
    println!("l_shipdate, unfiltered       : {shipdate_ms:7.1} ms (6000000 rows)");
    println!("l_shipdate, selection applied: {selected_ms:7.1} ms ({selected_rows} rows)");
    println!();

    // --- The engine's own scan ---------------------------------------------
    let mut reads = Vec::with_capacity(ITERATIONS);
    let mut kernels = Vec::with_capacity(ITERATIONS);
    let mut previous = 0u64;
    let mut rows = 0usize;
    for _ in 0..ITERATIONS {
        let (read, survivors) = engine_read(path, &plan);
        reads.push(read);
        kernels.push(Duration::from_nanos(plan.metrics().predicate_ns.saturating_sub(previous)));
        previous = plan.metrics().predicate_ns;
        rows = survivors;
    }
    let read_ms = ms(median(&mut reads));
    let kernel_ms = ms(median(&mut kernels));

    println!("--- the engine's own scan ---");
    println!("the whole scan      : {read_ms:7.1} ms   ({rows} rows survive)");
    println!("  of which the kernel: {kernel_ms:6.1} ms");
    println!("  of which the decode: {both_ms:6.1} ms");
    println!("  the rest is the mask, applied in the engine's domain");
    let _ = Flavor::CppSimd;
}
