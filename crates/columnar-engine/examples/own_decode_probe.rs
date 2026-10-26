//! What would the engine's own decode cost?
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind [spec 012](../../../specs/012-the-engine-owns-its-decode/spec.md).
//! It answers one question: **is the lazy scan's read path worth rewriting?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example own_decode_probe -- data/lineitem.parquet
//! ```
//!
//! The lazy path reads a Parquet file through Arrow. Arrow's reader is a thin
//! wrapper over the parquet crate's own reader: it decodes a column chunk into
//! a buffer and *moves* that buffer into an Arrow array. So the question is
//! **not** "is Arrow slow". It is:
//!
//! - what does the parquet decode cost on its own, with no Arrow in it?
//! - and how much of the scan is left over for the engine once it is paid?
//!
//! Measured on the same file, in the same process, warm:
//!
//! | | |
//! |---|---|
//! | **A** | the parquet crate's own decode of the two columns, into plain buffers |
//! | **B** | Arrow's decode of the same two columns |
//! | **C** | A, plus the engine's predicate over the decoded buffer, plus the compaction of the survivors into the engine's own column |
//! | **D** | the engine's own scan, as it is today |
//!
//! And A and C again at a **decode batch** small enough to stay in cache,
//! because a decode that streams 96 MB through the memory hierarchy is a
//! different measurement from one that never leaves L1.
//!
//! `filter-range` reads **two** columns: `l_discount`, because the predicates
//! need it, and `l_quantity`, because the aggregate sums it. Six million rows
//! each, twelve million `f64` in all.
//!
//! The file has no NULLs -- the manifest says `null_rate: 0.0` -- so the
//! parquet reader's values are already one per row and no padding is needed.
//! The probe says so out loud rather than assuming it quietly.
//!
//! The slices and the indexes below are into buffers this probe has just
//! filled, and their lengths are asserted immediately above every one of
//! them: the decoder reports how many values it wrote, the probe checks that
//! the count matches the rows it asked for, and only then indexes. The lint
//! is allowed here, and the assertion is the reason.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

use columnar_core::ComparisonOp;
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::ParquetSource;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;
use parquet::column::reader::{get_typed_column_reader, ColumnReader};
use parquet::data_type::DoubleType;
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 5;

/// The decode batches the probe reads at.
const BATCH_ROWS: [usize; 4] = [4_096, 16_384, 65_536, 262_144];

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The predicates, as the engine pushes them.
fn plan() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount = Pushdown::new(
        DISCOUNT,
        "l_discount",
        ComparisonOp::GtEq,
        columnar_core::ScalarValue::Float64(0.04),
    );
    discount.and(ComparisonOp::LtEq, columnar_core::ScalarValue::Float64(0.07));
    plan.push(discount);
    plan
}

/// A typed reader over one column.
type Typed = parquet::column::reader::ColumnReaderImpl<DoubleType>;

/// The reader over one column, typed.
fn typed(reader: ColumnReader) -> Typed {
    get_typed_column_reader::<DoubleType>(reader)
}

/// **A** — the parquet crate's own decode of the two columns.
///
/// The parquet reader decodes a column chunk into a plain buffer; the
/// definition levels say which rows are NULL, and with no NULLs in the file
/// the values are already one per row.
fn parquet_decode(path: &Path, batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let row_groups = reader.metadata().num_row_groups();

    let mut values: Vec<f64> = Vec::with_capacity(batch_rows);
    let mut levels: Vec<i16> = Vec::with_capacity(batch_rows);
    let mut rows = 0usize;

    let started = Instant::now();
    for group in 0..row_groups {
        let group = reader.get_row_group(group).expect("the row group reads");
        for &column in &[DISCOUNT, QUANTITY] {
            let mut reader = typed(group.get_column_reader(column).expect("the column reader"));

            loop {
                values.clear();
                values.reserve(batch_rows);
                levels.clear();
                levels.resize(batch_rows, 0);

                let (records, values_read, levels_read) = reader
                    .read_records(batch_rows, Some(&mut levels), None, &mut values)
                    .expect("the read runs");
                if records == 0 {
                    break;
                }
                assert_eq!(
                    values_read, records,
                    "this probe assumes a file without NULLs; the manifest says null_rate 0.0"
                );
                assert_eq!(levels_read, records, "the levels and the records disagree");
                rows += records;
                if records < batch_rows {
                    break;
                }
            }
        }
    }
    let elapsed = started.elapsed();
    std::hint::black_box(&values);
    (elapsed, rows)
}

/// **C** — the parquet crate's own decode, the engine's predicate over the
/// decoded buffer, and the compaction of the survivors into the engine's own
/// column.
///
/// This is the shape an engine-owned read path would have: the values are
/// decoded once, compared once, and the survivors are written once.
///
/// The two columns are read **in sequence**, not in lockstep: the discount
/// column of the row group first, with its masks held, and then the quantity
/// column, compacted through them. Reading them in lockstep keeps two page
/// readers alive at once, which is a cost the engine's own path does not pay
/// and which would make the comparison unfair.
fn parquet_decode_and_compact(path: &Path, batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let row_groups = reader.metadata().num_row_groups();
    let flavor = columnar_kernels::best();

    let mut values: Vec<f64> = Vec::with_capacity(batch_rows);
    let mut levels: Vec<i16> = Vec::with_capacity(batch_rows);
    let mut lower: Vec<u64> = Vec::new();
    let mut upper: Vec<u64> = Vec::new();
    let mut masks: Vec<u64> = Vec::new();
    let mut chunk: Vec<f64> = Vec::new();

    let started = Instant::now();
    for group in 0..row_groups {
        let group = reader.get_row_group(group).expect("the row group reads");

        // --- the predicate's column: decoded, compared, and dropped --------
        masks.clear();
        {
            let mut reader = typed(group.get_column_reader(DISCOUNT).expect("the column reader"));
            loop {
                values.clear();
                values.reserve(batch_rows);
                levels.clear();
                levels.resize(batch_rows, 0);

                let (records, values_read, levels_read) = reader
                    .read_records(batch_rows, Some(&mut levels), None, &mut values)
                    .expect("the read runs");
                if records == 0 {
                    break;
                }
                assert_eq!(values_read, records, "this probe assumes a file without NULLs");
                assert_eq!(levels_read, records, "the levels and the records disagree");

                let words = records.div_ceil(64);
                lower.clear();
                lower.resize(words, 0);
                upper.clear();
                upper.resize(words, 0);
                columnar_kernels::compare_f64_into(
                    flavor,
                    &values[..records],
                    ComparisonOp::GtEq,
                    0.04,
                    None,
                    &mut lower,
                );
                columnar_kernels::compare_f64_into(
                    flavor,
                    &values[..records],
                    ComparisonOp::LtEq,
                    0.07,
                    None,
                    &mut upper,
                );
                masks.extend(lower.iter().zip(upper.iter()).map(|(a, b)| a & b));
                if records < batch_rows {
                    break;
                }
            }
        }

        // --- the materialised column: decoded, and compacted ---------------
        {
            let mut reader = typed(group.get_column_reader(QUANTITY).expect("the column reader"));
            let mut offset = 0usize;
            loop {
                values.clear();
                values.reserve(batch_rows);
                levels.clear();
                levels.resize(batch_rows, 0);

                let (records, values_read, levels_read) = reader
                    .read_records(batch_rows, Some(&mut levels), None, &mut values)
                    .expect("the read runs");
                if records == 0 {
                    break;
                }
                assert_eq!(values_read, records, "this probe assumes a file without NULLs");
                assert_eq!(levels_read, records, "the levels and the records disagree");

                for word_index in 0..records.div_ceil(64) {
                    let base = word_index * 64;
                    let mut bits = masks[offset + word_index];
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        let row = base + bit;
                        if row < records {
                            chunk.push(values[row]);
                        }
                        bits &= bits - 1;
                    }
                }
                offset += records.div_ceil(64);
                if records < batch_rows {
                    break;
                }
            }
        }
    }
    let elapsed = started.elapsed();
    let survivors = chunk.len();
    std::hint::black_box(&chunk);
    (elapsed, survivors)
}

/// **B** — Arrow's decode of the same two columns.
///
/// The same work as A, through Arrow's reader instead of the parquet crate's
/// own. Arrow's reader is a thin wrapper over the same code -- it decodes a
/// column chunk into a buffer and *moves* that buffer into an Arrow array --
/// so the difference between A and B is the whole of Arrow's cost.
fn arrow_decode(path: &Path, batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask =
        ProjectionMask::roots(builder.parquet_schema(), [DISCOUNT, QUANTITY].iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(batch_rows)
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

/// **D** — the engine's own scan, end to end.
fn engine_scan(path: &Path) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset =
        source.read(&[QUANTITY], &plan(), columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// Repeats a measurement and returns the median.
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
    println!("iterations: {ITERATIONS}\n");

    println!("--- A: the parquet crate's own decode, no Arrow ---");
    for batch_rows in BATCH_ROWS {
        let (elapsed, rows) = repeat(|| parquet_decode(path, batch_rows));
        println!("  batch {batch_rows:>6} : {:7.2} ms   ({rows} rows)", ms(elapsed));
    }
    println!();

    println!("--- B: Arrow's decode, the same two columns ---");
    for batch_rows in BATCH_ROWS {
        let (elapsed, rows) = repeat(|| arrow_decode(path, batch_rows));
        println!("  batch {batch_rows:>6} : {:7.2} ms   ({rows} rows)", ms(elapsed));
    }
    println!();

    println!("--- D: the engine's own scan, as it is today ---");
    let (scan, rows) = repeat(|| engine_scan(path));
    println!("  the whole scan    : {:7.2} ms   ({rows} rows survive)", ms(scan));
    println!();

    println!("--- C: the decode, the predicate and the compaction, without Arrow ---");
    for batch_rows in BATCH_ROWS {
        let (elapsed, survivors) = repeat(|| parquet_decode_and_compact(path, batch_rows));
        println!("  batch {batch_rows:>6} : {:7.2} ms   ({survivors} rows survive)", ms(elapsed));
    }
}
