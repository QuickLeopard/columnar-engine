//! End-to-end tests for the dataset generator.
//!
//! Every test writes a real Parquet file and reads it back, because the
//! claims this crate makes -- the schema, the dictionary encoding, the null
//! injection, the value ranges, the determinism -- are claims about a file
//! on disk, not about a value in memory.

use arrow::array::{Array, ArrayRef, DictionaryArray, Float64Array, Int64Array, StringArray};
use arrow::datatypes::DataType as ArrowDataType;
use arrow::datatypes::Int32Type;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use columnar_testdata::generator::{self, DatasetGenerator};
use columnar_testdata::schema::{self, COLUMNS, DICTIONARY_COLUMNS};
use columnar_testdata::writer::{write_dataset, BATCH_ROWS, ROW_GROUP_ROWS};
use columnar_testdata::{generate, DatasetManifest, DatasetSpec};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Encoding;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The rows used by the tests that do not need a scale factor.
const TEST_ROWS: u64 = 100_000;

/// A path inside the test target directory.
fn test_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Reads the Arrow schema the file was written with.
fn read_schema(path: &Path) -> SchemaRef {
    let file = File::open(path).expect("the dataset file exists");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("the file is a parquet file");
    builder.schema().clone()
}

/// Reads every batch of the file.
fn read_batches(path: &Path) -> Vec<RecordBatch> {
    let file = File::open(path).expect("the dataset file exists");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("the file is a parquet file");
    let reader = builder.with_batch_size(8192).build().expect("the reader builds");
    reader
        .collect::<Result<Vec<RecordBatch>, arrow::error::ArrowError>>()
        .expect("the file reads back")
}

/// The encodings the file uses for one column, taken from the Parquet
/// metadata rather than from the Arrow schema.
///
/// The Arrow schema says what the column *is*; the Parquet metadata says
/// how it is *stored*. The dictionary claim is about the second one.
fn column_encodings(path: &Path, column: usize) -> Vec<Encoding> {
    let file = File::open(path).expect("the dataset file exists");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("the file is a parquet file");
    let metadata = builder.metadata().clone();
    let mut encodings = Vec::new();
    for row_group in metadata.row_groups() {
        let chunk = row_group.columns().get(column).expect("the row group has the column");
        encodings.extend(chunk.encodings().iter().copied());
    }
    encodings
}

/// The decoded values of a dictionary-encoded column.
fn dictionary_strings(array: &ArrayRef) -> Vec<Option<String>> {
    let dictionary = array
        .as_any()
        .downcast_ref::<DictionaryArray<Int32Type>>()
        .expect("the column is dictionary-encoded");
    let values = dictionary
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("the dictionary holds strings");
    let keys = dictionary.keys();
    (0..dictionary.len())
        .map(|row| {
            if keys.is_null(row) {
                None
            } else {
                let key = usize::try_from(keys.value(row)).expect("the key is a valid index");
                Some(values.value(key).to_string())
            }
        })
        .collect()
}

/// The `l_comment` column of a file, decoded and concatenated.
fn comment_text(path: &Path) -> String {
    let mut out = String::new();
    for batch in read_batches(path) {
        let array = batch
            .column(15)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("l_comment is a string column");
        for row in 0..array.len() {
            if array.is_null(row) {
                out.push_str("<null>\n");
            } else {
                out.push_str(array.value(row));
                out.push('\n');
            }
        }
    }
    out
}

/// Writes a dataset of `rows` rows with the given spec.
fn write(spec: &DatasetSpec, rows: u64, name: &str) -> (PathBuf, DatasetManifest) {
    let path = test_path(name);
    let manifest =
        write_dataset(spec, rows, &path, ROW_GROUP_ROWS).expect("the dataset is written");
    (path, manifest)
}

#[test]
fn the_same_seed_produces_byte_identical_files() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x00C0_FFEE, null_rate: 0.1 };
    let (first, _) = write(&spec, TEST_ROWS, "determinism_a.parquet");
    let (second, _) = write(&spec, TEST_ROWS, "determinism_b.parquet");

    let first_bytes = std::fs::read(&first).expect("the first file reads");
    let second_bytes = std::fs::read(&second).expect("the second file reads");

    assert_eq!(first_bytes.len(), second_bytes.len(), "the two files have different sizes");
    assert!(first_bytes == second_bytes, "the two files differ byte for byte");
}

#[test]
fn the_same_seed_produces_the_same_rows() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x00C0_FFEE, null_rate: 0.1 };
    let (first, _) = write(&spec, TEST_ROWS, "content_a.parquet");
    let (second, _) = write(&spec, TEST_ROWS, "content_b.parquet");

    // The highest-cardinality column is the strongest check available: if
    // the random stream drifted at all, the comments would differ.
    assert_eq!(comment_text(&first), comment_text(&second));
}

#[test]
fn a_different_seed_produces_different_rows() {
    let base = DatasetSpec { scale_factor: 1, seed: 1, null_rate: 0.0 };
    let other = DatasetSpec { seed: 2, ..base.clone() };
    let (first, _) = write(&base, 10_000, "seed_a.parquet");
    let (second, _) = write(&other, 10_000, "seed_b.parquet");

    assert_ne!(comment_text(&first), comment_text(&second));
}

#[test]
fn the_written_schema_matches_the_contract() {
    let spec = DatasetSpec::default();
    let (path, manifest) = write(&spec, 1_000, "schema.parquet");
    let schema = read_schema(&path);

    let expected_names: Vec<&str> = COLUMNS.iter().map(|column| column.name).collect();
    let actual_names: Vec<&str> =
        schema.fields().iter().map(|field| field.name().as_str()).collect();
    assert_eq!(actual_names, expected_names, "the column order is the contract");
    assert_eq!(manifest.columns, schema::column_names());

    for (field, column) in schema.fields().iter().zip(COLUMNS.iter()) {
        let expected = schema::arrow_data_type(column);
        assert_eq!(field.data_type(), &expected, "column {} has the wrong type", column.name);
    }

    // The engine's vocabulary must agree with the Arrow schema.
    assert_eq!(schema::engine_data_type("l_orderkey"), Some(columnar_core::DataType::Int64));
    assert_eq!(schema::engine_data_type("l_quantity"), Some(columnar_core::DataType::Float64));
    assert_eq!(schema::engine_data_type("l_shipdate"), Some(columnar_core::DataType::Utf8));
    assert_eq!(schema::engine_data_type("l_nope"), None);
}

#[test]
fn the_string_columns_are_dictionary_encoded_on_disk() {
    let spec = DatasetSpec::default();
    let (path, _) = write(&spec, 10_000, "dictionary.parquet");
    let schema = read_schema(&path);

    for (index, column) in COLUMNS.iter().enumerate() {
        let field = schema.field(index);
        if column.dictionary {
            assert!(
                matches!(field.data_type(), ArrowDataType::Dictionary(_, _)),
                "column {} should be a dictionary in the Arrow schema",
                column.name
            );

            let encodings = column_encodings(&path, index);
            assert!(
                encodings.contains(&Encoding::RLE_DICTIONARY),
                "column {} is not RLE_DICTIONARY encoded, only {encodings:?}",
                column.name
            );

            // The column must decode as a dictionary, not as a flat string
            // column that happens to have been dictionary encoded.
            let batches = read_batches(&path);
            let array = batches.first().expect("the file has at least one batch").column(index);
            assert!(
                array.as_any().is::<DictionaryArray<Int32Type>>(),
                "column {} does not decode as a dictionary",
                column.name
            );
        } else {
            assert!(
                !matches!(field.data_type(), ArrowDataType::Dictionary(_, _)),
                "column {} should not be a dictionary",
                column.name
            );
        }
    }
}

#[test]
fn the_comment_column_is_not_dictionary_encoded() {
    let spec = DatasetSpec::default();
    let (path, _) = write(&spec, 10_000, "comment_plain.parquet");

    let index = COLUMNS
        .iter()
        .position(|column| column.name == "l_comment")
        .expect("l_comment is a column");
    let encodings = column_encodings(&path, index);

    assert!(
        encodings.contains(&Encoding::PLAIN),
        "l_comment should be plain encoded, only {encodings:?}"
    );
    assert!(
        !encodings.contains(&Encoding::RLE_DICTIONARY),
        "l_comment should not be dictionary encoded, only {encodings:?}"
    );
}

#[test]
fn the_dictionary_columns_are_the_seven_low_cardinality_ones() {
    assert_eq!(
        DICTIONARY_COLUMNS,
        [
            "l_returnflag",
            "l_linestatus",
            "l_shipdate",
            "l_commitdate",
            "l_receiptdate",
            "l_shipinstruct",
            "l_shipmode",
        ]
    );
    assert_eq!(DICTIONARY_COLUMNS.len(), 7);
    assert_eq!(COLUMNS.iter().filter(|column| column.dictionary).count(), DICTIONARY_COLUMNS.len());
}

#[test]
fn the_null_rate_is_injected_into_the_discount_column() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x5EED, null_rate: 0.1 };
    let (path, _) = write(&spec, TEST_ROWS, "nulls.parquet");

    let mut total = 0u64;
    let mut nulls = 0u64;
    for batch in read_batches(&path) {
        let array = batch
            .column(6)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_discount is a float column");
        total += u64::try_from(array.len()).expect("the length fits in a u64");
        nulls += u64::try_from(array.null_count()).expect("the null count fits in a u64");
    }

    assert_eq!(total, TEST_ROWS);
    let nulls = f64::from(u32::try_from(nulls).expect("the null count fits in a u32"));
    let total = f64::from(u32::try_from(total).expect("the row count fits in a u32"));
    let fraction = nulls / total;
    assert!(
        (0.08..=0.12).contains(&fraction),
        "the null fraction is {fraction}, expected roughly 0.1"
    );
}

#[test]
fn a_zero_null_rate_injects_no_nulls() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x5EED, null_rate: 0.0 };
    let (path, _) = write(&spec, 10_000, "no_nulls.parquet");

    for batch in read_batches(&path) {
        let discount = batch
            .column(6)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_discount is a float column");
        let comment = batch
            .column(15)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("l_comment is a string column");
        assert_eq!(discount.null_count(), 0, "l_discount must not be null");
        assert_eq!(comment.null_count(), 0, "l_comment must not be null");
    }
}

#[test]
fn the_values_are_in_range() {
    let spec = DatasetSpec::default();
    let (path, _) = write(&spec, 50_000, "ranges.parquet");

    let ship_dates = generator::SHIP_DATE_START;
    let ship_dates_end = generator::SHIP_DATE_END;
    let first_day = generator::days_from_civil(ship_dates.0, ship_dates.1, ship_dates.2);
    let last_day = generator::days_from_civil(ship_dates_end.0, ship_dates_end.1, ship_dates_end.2);

    for batch in read_batches(&path) {
        let quantity = batch
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_quantity is a float column");
        for value in quantity.values() {
            assert!((1.0..=50.0).contains(value), "l_quantity is {value}");
        }

        let discount = batch
            .column(6)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_discount is a float column");
        for value in discount.values() {
            assert!((0.0..=0.1).contains(value), "l_discount is {value}");
        }

        let tax = batch
            .column(7)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("l_tax is a float column");
        for value in tax.values() {
            assert!((0.0..=0.08).contains(value), "l_tax is {value}");
        }

        let linenumber = batch
            .column(3)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("l_linenumber is an int column");
        for value in linenumber.values() {
            assert!((1..=7).contains(value), "l_linenumber is {value}");
        }

        let partkey = batch
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("l_partkey is an int column");
        for value in partkey.values() {
            assert!((1..=5_000_000).contains(value), "l_partkey is {value}");
        }

        let suppkey = batch
            .column(2)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("l_suppkey is an int column");
        for value in suppkey.values() {
            assert!((1..=10_000).contains(value), "l_suppkey is {value}");
        }

        // The dates are UTF-8 strings in YYYY-MM-DD form.
        let shipdates = dictionary_strings(batch.column(10));
        for value in shipdates {
            let text = value.expect("l_shipdate is not null");
            assert!(is_iso_date(&text), "l_shipdate is {text}");
            let day = parse_iso_date(&text).expect("l_shipdate parses");
            assert!((first_day..=last_day).contains(&day), "l_shipdate {text} is out of range");
        }
    }
}

#[test]
fn the_commit_and_receipt_dates_follow_the_ship_date() {
    let spec = DatasetSpec::default();
    let (path, _) = write(&spec, 10_000, "date_offsets.parquet");

    for batch in read_batches(&path) {
        let shipdates = dictionary_strings(batch.column(10));
        let commitdates = dictionary_strings(batch.column(11));
        let receiptdates = dictionary_strings(batch.column(12));

        for row in 0..shipdates.len() {
            let ship_text = shipdates
                .get(row)
                .and_then(|value| value.as_deref())
                .expect("the ship date is there");
            let commit_text = commitdates
                .get(row)
                .and_then(|value| value.as_deref())
                .expect("the commit date is there");
            let receipt_text = receiptdates
                .get(row)
                .and_then(|value| value.as_deref())
                .expect("the receipt date is there");
            let ship = parse_iso_date(ship_text).expect("the ship date parses");
            let commit = parse_iso_date(commit_text).expect("the commit date parses");
            let receipt = parse_iso_date(receipt_text).expect("the receipt date parses");

            assert!(
                (0..=121).contains(&(commit - ship)),
                "the commit date trails the ship date by {} days",
                commit - ship
            );
            assert!(
                (1..=30).contains(&(receipt - ship)),
                "the receipt date trails the ship date by {} days",
                receipt - ship
            );
        }
    }
}

#[test]
fn the_manifest_round_trips_through_json() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x5EED, null_rate: 0.1 };
    let (path, manifest) = write(&spec, 1_000, "manifest.parquet");

    manifest.write_json().expect("the manifest is written");
    let read = DatasetManifest::read_json(&path).expect("the manifest reads back");

    assert_eq!(manifest, read);
    assert_eq!(read.rows, 1_000);
    assert!(
        std::path::Path::new(&read.path).is_absolute(),
        "the manifest records an absolute path, got {}",
        read.path
    );
    assert_eq!(read.seed, 0x5EED);
    assert_eq!(read.scale_factor, 1);
    assert!(read.bytes > 0);
    assert!(read.provenance.contains("NOT the official TPC-H SF-1 file"));
    assert_eq!(read.columns, schema::column_names());
}

#[test]
fn a_small_dataset_is_generated_quickly() {
    let spec = DatasetSpec::default();
    let path = test_path("small.parquet");

    let started = Instant::now();
    let manifest =
        write_dataset(&spec, TEST_ROWS, &path, ROW_GROUP_ROWS).expect("the dataset is written");
    let elapsed = started.elapsed();

    assert_eq!(manifest.rows, TEST_ROWS);
    assert!(manifest.bytes > 0);
    assert!(elapsed.as_secs() < 60, "writing {TEST_ROWS} rows took {elapsed:?}");
}

#[test]
fn the_row_count_follows_the_scale_factor() {
    assert_eq!(schema::rows_for_scale_factor(1), 6_000_000);
    assert_eq!(schema::rows_for_scale_factor(10), 60_000_000);
    assert_eq!(schema::rows_for_scale_factor(0), 0);
}

#[test]
fn generate_writes_the_scale_factor_row_count() {
    // Scale factor zero is the only end-to-end run of `generate` that is
    // cheap enough for a test; the row count rule itself is checked above.
    let spec = DatasetSpec { scale_factor: 0, seed: 0x5EED, null_rate: 0.0 };
    let path = test_path("generate.parquet");
    let manifest = generate(&spec, &path).expect("the dataset is written");

    assert_eq!(manifest.rows, 0);
    assert_eq!(manifest.columns, schema::column_names());
    assert!(path.exists());
}

#[test]
fn an_out_of_range_null_rate_is_rejected() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x5EED, null_rate: 1.5 };
    let path = test_path("invalid.parquet");
    let error = write_dataset(&spec, 10, &path, ROW_GROUP_ROWS).expect_err("the spec is rejected");

    assert!(
        matches!(error, columnar_testdata::DatasetError::InvalidSpec(_)),
        "the error is {error:?}"
    );
}

#[test]
fn the_generator_streams_in_bounded_batches() {
    let spec = DatasetSpec::default();
    let mut generator = DatasetGenerator::new(&spec, 10).expect("the generator builds");

    let batch = generator.next_batch(BATCH_ROWS).expect("the batch builds");
    assert_eq!(batch.num_rows(), 10);
    assert_eq!(batch.num_columns(), COLUMNS.len());
    assert_eq!(generator.rows_emitted(), 10);

    let batch = generator.next_batch(BATCH_ROWS).expect("the batch builds");
    assert_eq!(batch.num_rows(), 0, "the generator stops at its row count");
}

#[test]
fn the_row_group_size_is_bounded() {
    let spec = DatasetSpec::default();
    // One and a half row groups, so that the split is actually exercised.
    let (path, _) = write(&spec, 1_500_000, "row_groups.parquet");

    let file = File::open(&path).expect("the dataset file exists");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("the file is a parquet file");
    let metadata = builder.metadata().clone();

    let row_groups = metadata.row_groups();
    assert_eq!(row_groups.len(), 2, "the dataset should be split into two row groups");

    let limit = i64::try_from(ROW_GROUP_ROWS).expect("the row group size fits in an i64");
    for row_group in row_groups {
        let rows = row_group.num_rows();
        assert!(rows <= limit, "a row group holds {rows} rows, which is over the limit");
    }
    assert_eq!(
        row_groups.first().map(parquet::file::metadata::RowGroupMetaData::num_rows),
        Some(limit),
        "the first row group should be full"
    );
    assert_eq!(
        row_groups.get(1).map(parquet::file::metadata::RowGroupMetaData::num_rows),
        Some(500_000),
        "the last row group should hold the remainder"
    );
}

/// How many row groups the file on disk has.
fn row_group_count(path: &Path) -> usize {
    let file = File::open(path).expect("the dataset file exists");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).expect("the file is a parquet file");
    builder.metadata().row_groups().len()
}

#[test]
fn the_row_group_size_is_the_knob() {
    let spec = DatasetSpec::default();
    let path = test_path("row_group_knob.parquet");

    // A fifth of the default, so that the count is a fifth too.
    let rows_per_group = ROW_GROUP_ROWS / 5;
    let manifest =
        write_dataset(&spec, 1_500_000, &path, rows_per_group).expect("the dataset is written");

    // One and a half groups of two hundred thousand rows: three groups.
    let expected = 1_500_000_u64.div_ceil(u64::try_from(rows_per_group).expect("the size fits"));
    assert_eq!(
        row_group_count(&path),
        usize::try_from(expected).expect("the count fits in a usize"),
        "the file should have the row groups the knob asked for"
    );

    // The manifest says what was asked for and what was written, so a reader
    // of the manifest knows the file's shape without opening it.
    assert_eq!(manifest.row_group_rows, rows_per_group);
    assert_eq!(manifest.row_groups, expected);
}

#[test]
fn the_default_row_group_size_does_not_move() {
    let spec = DatasetSpec::default();
    let path = test_path("row_group_default.parquet");

    let manifest =
        write_dataset(&spec, 1_500_000, &path, ROW_GROUP_ROWS).expect("the dataset is written");

    assert_eq!(manifest.row_group_rows, ROW_GROUP_ROWS);
    assert_eq!(manifest.row_groups, 2);
    assert_eq!(row_group_count(&path), 2);
}

#[test]
fn the_manifest_records_the_row_group_count_of_the_generated_file() {
    // Scale factor zero is the only end-to-end run of `generate` that is
    // cheap enough for a test; the row-group rule itself is checked above.
    let spec = DatasetSpec { scale_factor: 0, seed: 0x5EED, null_rate: 0.0 };
    let path = test_path("generate_row_groups.parquet");
    let manifest = columnar_testdata::generate_with_row_groups(&spec, &path, ROW_GROUP_ROWS / 4)
        .expect("the dataset is written");

    assert_eq!(manifest.row_group_rows, ROW_GROUP_ROWS / 4);
    assert_eq!(manifest.row_groups, 0, "an empty file has no row groups");
}

/// Whether a string looks like a `YYYY-MM-DD` date.
fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// Parses a `YYYY-MM-DD` date into a day number.
fn parse_iso_date(text: &str) -> Option<i64> {
    if !is_iso_date(text) {
        return None;
    }
    let year = text.get(0..4)?.parse::<i32>().ok()?;
    let month = text.get(5..7)?.parse::<u32>().ok()?;
    let day = text.get(8..10)?.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let day_number = generator::days_from_civil(year, month, day);
    let (back_year, back_month, back_day) = generator::civil_from_days(day_number);
    if (back_year, back_month, back_day) != (year, month, day) {
        return None;
    }
    Some(day_number)
}
