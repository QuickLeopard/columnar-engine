//! The per-row-group cost, attributed.
//!
//! This is a diagnostic, not part of the engine, and it is the measurement
//! behind [ADR-0023](../../../docs/adr/0023-the-per-row-group-cost.md).
//! It answers one question: **where does the engine's cost per row group
//! come from?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example row_group_cost_probe -- \
//!     data/lineitem.parquet --out report.json
//! ```
//!
//! Spec 015 measured the cost itself: on files with six, twelve and
//! twenty-four row groups, the engine's one-thread figure rises by about
//! **0.8 ms a row group**, while `DuckDB`'s, `Polars`' and the `DataFusion`
//! peer's do not move at all. They read the same bytes, so the engine's
//! *read* is what got slower.
//!
//! There are three places it could be:
//!
//! 1. **The parquet crate's own read**, one row group at a time — the crate
//!    opens a column reader per row group, and reads and decodes that row
//!    group's dictionary pages.
//! 2. **The engine's pushdown**, which compares a dictionary-encoded column
//!    once per distinct value, per batch — and a row group is a batch
//!    boundary.
//! 3. **The engine's materialisation**, which copies the survivors out of
//!    the batch.
//!
//! So the probe measures the **crate's read on its own**, with the same
//! projection and no pushdown and no materialisation; the crate's read with
//! the pushdown on top; and the **engine's scan**. The differences between
//! them are the engine's own work. The three files' rates then say whether
//! the cost is the crate's or the engine's.
//!
//! It also reads the file's metadata: each row group's compressed bytes, and
//! the dictionary pages' share of them, so the cost can be compared with
//! the bytes that were read.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

use columnar_core::{ComparisonOp, ScalarValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::ParquetSource;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReaderBuilder, RowSelection, RowSelector};
use parquet::arrow::ProjectionMask;
use parquet::file::reader::{FileReader, SerializedFileReader};

/// The file's root index of `l_extendedprice`.
const EXTENDEDPRICE: usize = 3;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;
/// The file's root index of `l_shipdate`.
const SHIPDATE: usize = 10;

/// The batch size the lazy path uses.
const READER_BATCH_ROWS: usize = 65_536;

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

/// The parquet crate's read of some columns, on its own.
///
/// The reader, with a projection mask and **no** row filter: the batches are
/// counted and dropped. This is the crate's share of the cost.
fn crate_read(path: &Path, columns: &[usize]) -> (Duration, usize) {
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

/// The crate's read, with the engine's pushdown on top.
///
/// The pushdown is the engine's own work over the batch the crate produced:
/// the mask, and the dictionary comparisons behind it. Nothing is
/// materialised.
fn crate_read_with_pushdown(
    path: &Path,
    columns: &[usize],
    plan: &PushdownPlan,
) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READER_BATCH_ROWS)
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

/// The parquet crate's read of some columns, with the string columns read as
/// **dictionary arrays**.
///
/// The crate's plain byte-array reader expands the dictionary into every
/// value: one heap allocation per value, and the dictionary itself is built
/// again for every row group. Its dictionary reader keeps the indices and the
/// dictionary, and builds the dictionary once per row group as a contiguous
/// Arrow array.
fn crate_read_as_dictionary(path: &Path, columns: &[usize]) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let plain = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let schema = plain.schema().clone();
    let fields: Vec<arrow::datatypes::FieldRef> = schema
        .fields()
        .iter()
        .map(|field| {
            if field.data_type() == &arrow::datatypes::DataType::Utf8 {
                arrow::datatypes::Field::new(
                    field.name(),
                    arrow::datatypes::DataType::Dictionary(
                        Box::new(arrow::datatypes::DataType::Int32),
                        Box::new(arrow::datatypes::DataType::Utf8),
                    ),
                    field.is_nullable(),
                )
                .into()
            } else {
                field.clone()
            }
        })
        .collect();
    let schema = arrow::datatypes::Schema::new(fields);

    let file = File::open(path).expect("the file opens");
    let options = parquet::arrow::arrow_reader::ArrowReaderOptions::new()
        .with_schema(std::sync::Arc::new(schema));
    let metadata = parquet::arrow::arrow_reader::ArrowReaderMetadata::load(&file, options)
        .expect("the metadata loads");
    let builder = ParquetRecordBatchReaderBuilder::new_with_metadata(file, metadata);
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

/// The crate's read of some columns, **skipping every row**.
///
/// The pages are read and the dictionary is built, but no value is decoded and
/// nothing is copied out. Whatever the per-row-group cost turns out to be, it
/// is in one of those two, and this separates them.
fn crate_read_skipping(path: &Path, columns: &[usize], rows: u64) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
    let selection = RowSelection::from(vec![RowSelector::skip(usize::try_from(rows).unwrap_or(0))]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(READER_BATCH_ROWS)
        .with_row_selection(selection)
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

/// The crate's read of some columns, with a given batch size.
///
/// A row group is a batch boundary, so the batch size decides how many
/// batches a row group holds — and whether the last batch of a row group is
/// a full one.
fn crate_read_with_batch(path: &Path, columns: &[usize], batch_rows: usize) -> (Duration, usize) {
    let file = File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let mask = ProjectionMask::roots(builder.parquet_schema(), columns.iter().copied());
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

/// The engine's own lazy scan, with a given reader batch size.
fn engine_scan_with_batch(
    path: &Path,
    plan: &PushdownPlan,
    materialised: &[usize],
    batch_rows: usize,
) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source
        .read_with_batch_rows(materialised, plan, columnar_kernels::best(), 1, batch_rows)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// The engine's own lazy scan, end to end.
fn engine_scan(path: &Path, plan: &PushdownPlan, materialised: &[usize]) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset =
        source.read(materialised, plan, columnar_kernels::best(), 1).expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// What the file's metadata says about the projected columns.
///
/// For each row group: how many compressed bytes the columns hold, and how
/// many of those bytes are the **dictionary pages** — the pages that exist
/// because the writer wrote a dictionary and then fell back to plain. The
/// dictionary page's size is the distance from its offset to the data
/// page's, which is its header and its body.
fn row_group_bytes(path: &Path, columns: &[usize]) -> (Vec<u64>, Vec<u64>) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let metadata = reader.metadata();
    let mut compressed = Vec::with_capacity(metadata.num_row_groups());
    let mut dictionary = Vec::with_capacity(metadata.num_row_groups());

    for group in metadata.row_groups() {
        let mut bytes = 0u64;
        let mut dict = 0u64;
        for column in columns {
            let chunk = group.column(*column);
            bytes += u64::try_from(chunk.compressed_size().max(0)).unwrap_or(0);
            if let Some(dictionary_offset) = chunk.dictionary_page_offset() {
                let data_offset = chunk.data_page_offset();
                let span = data_offset.saturating_sub(dictionary_offset).max(0);
                dict += u64::try_from(span).unwrap_or(0);
            }
        }
        compressed.push(bytes);
        dictionary.push(dict);
    }

    (compressed, dictionary)
}

/// What each row group's pages look like, and how long the pages take to read.
///
/// The pages are read with the crate's own page reader — the headers and the
/// decompressed bytes — and **nothing is decoded**. The dictionary's size in
/// values is the number the crate builds its dictionary from, once per row
/// group.
fn page_shape(path: &Path, column: usize) -> (Vec<u32>, Vec<usize>, Duration) {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    let mut dictionary_values = Vec::with_capacity(reader.num_row_groups());
    let mut data_pages = Vec::with_capacity(reader.num_row_groups());
    let started = Instant::now();
    for group in 0..reader.num_row_groups() {
        let group = reader.get_row_group(group).expect("the row group reads");
        let mut pages = group.get_column_page_reader(column).expect("the page reader opens");
        let mut values = 0u32;
        let mut count = 0usize;
        while let Some(page) = pages.get_next_page().expect("the page reads") {
            match page {
                parquet::column::page::Page::DictionaryPage { num_values, .. } => {
                    values = num_values;
                }
                _ => count += 1,
            }
        }
        dictionary_values.push(values);
        data_pages.push(count);
    }
    (dictionary_values, data_pages, started.elapsed())
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

/// The file's row-group count.
fn row_groups(path: &Path) -> usize {
    let file = File::open(path).expect("the file opens");
    let reader = SerializedFileReader::new(file).expect("the reader opens");
    reader.metadata().num_row_groups()
}

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
    let groups = row_groups(path);
    let projection = [DISCOUNT, SHIPDATE];

    println!("file: {}", path.display());
    println!("row groups: {groups}, iterations: {ITERATIONS}\n");

    // --- The crate's read, on its own ---------------------------------------
    let discount_ms = ms(repeat(|| crate_read(path, &[DISCOUNT]).0));
    let shipdate_ms = ms(repeat(|| crate_read(path, &[SHIPDATE]).0));
    let price_ms = ms(repeat(|| crate_read(path, &[EXTENDEDPRICE]).0));
    let both_ms = ms(repeat(|| crate_read(path, &projection).0));
    let shipdate_dict_ms = ms(repeat(|| crate_read_as_dictionary(path, &[SHIPDATE]).0));
    let both_dict_ms = ms(repeat(|| crate_read_as_dictionary(path, &projection).0));
    println!("--- the parquet crate's read, no pushdown, no materialisation ---");
    println!("l_discount only   : {discount_ms:7.2} ms");
    println!("l_shipdate only   : {shipdate_ms:7.2} ms");
    println!("l_extendedprice   : {price_ms:7.2} ms");
    println!("both columns      : {both_ms:7.2} ms");
    println!("--- the same read, with the strings as dictionary arrays ---");
    println!("l_shipdate only   : {shipdate_dict_ms:7.2} ms");
    println!("both columns      : {both_dict_ms:7.2} ms");
    let skipping_ms = ms(repeat(|| crate_read_skipping(path, &projection, 6_000_000).0));
    let _ = skipping_ms;
    let one_batch_ms = ms(repeat(|| crate_read_with_batch(path, &projection, 1_000_000).0));
    println!("--- the same read, with every row skipped ---");
    println!("both columns      : {skipping_ms:7.2} ms");
    println!("--- the same read, with one batch a row group ---");
    println!("both columns      : {one_batch_ms:7.2} ms");
    println!();

    // --- The crate's read, with the pushdown on top -------------------------
    let pushdown_ms = ms(repeat(|| crate_read_with_pushdown(path, &projection, &plan).0));
    println!("--- the crate's read, with the engine's pushdown on top ---");
    println!("both columns      : {pushdown_ms:7.2} ms");
    println!();

    // --- The engine's own scan ----------------------------------------------
    let engine_predicates_ms = ms(repeat(|| engine_scan(path, &plan, &[]).0));
    let engine_full_ms = ms(repeat(|| engine_scan(path, &plan, &[EXTENDEDPRICE]).0));
    println!("--- the engine's own scan ---");
    println!("the predicates    : {engine_predicates_ms:7.2} ms");
    println!("and the price     : {engine_full_ms:7.2} ms");
    println!("--- the engine's own scan, by reader batch size ---");
    for batch_rows in [12_500usize, 25_000, 31_250, 50_000, 62_500, 65_536, 125_000, 250_000] {
        let elapsed = ms(repeat(|| engine_scan_with_batch(path, &plan, &[], batch_rows).0));
        println!("batch {batch_rows:>7} rows : {elapsed:7.2} ms");
    }
    println!();

    // --- The file's own shape ------------------------------------------------
    let (compressed, dictionary) = row_group_bytes(path, &projection);
    let compressed_total: u64 = compressed.iter().sum();
    let dictionary_total: u64 = dictionary.iter().sum();
    println!("--- the file's shape, for the two predicate columns ---");
    println!("compressed bytes  : {compressed_total} over {groups} row groups");
    println!("dictionary bytes  : {dictionary_total} over {groups} row groups");
    println!();

    // --- The pages, and what the crate does with them ------------------------
    println!("--- the pages of each column, and the cost of reading them ---");
    for (name, column) in [("l_discount", DISCOUNT), ("l_shipdate", SHIPDATE)] {
        let (values, pages, elapsed) = page_shape(path, column);
        let total_values: u32 = values.iter().sum();
        let total_pages: usize = pages.iter().sum();
        println!(
            "{name:<12}: {total_values} dictionary values over {groups} row groups \
             ({:.0} a group), {total_pages} data pages, pages read in {:.2} ms",
            f64::from(
                u16::try_from(total_values / u32::try_from(groups).unwrap_or(1)).unwrap_or(0)
            ),
            ms(elapsed)
        );
    }
    println!();

    let report = serde_json::json!({
        "probe": "row-group-cost",
        "file": path.display().to_string(),
        "row_groups": groups,
        "iterations": ITERATIONS,
        "measurements_ms": {
            "crate_discount": discount_ms,
            "crate_shipdate": shipdate_ms,
            "crate_price": price_ms,
            "crate_both": both_ms,
            "crate_with_pushdown": pushdown_ms,
            "engine_predicates": engine_predicates_ms,
            "engine_full": engine_full_ms,
        },
        "bytes": {
            "compressed_total": compressed_total,
            "dictionary_total": dictionary_total,
        },
    });

    println!("--- the report ---");
    println!("{report}");
    if let Some(out) = out_path {
        std::fs::write(&out, format!("{report}\n")).expect("the report is written");
        println!("report: {out}");
    }
}
