//! End-to-end tests for the `orders` table.
//!
//! The same claims the `lineitem` tests make, made about the second table:
//! the schema, the determinism, the null injection, the value ranges --
//! and the two that matter most for the join: the order keys are distinct,
//! and generating `orders` does not move a single `lineitem` row.
//!
//! Every test writes a real Parquet file and reads it back, because the
//! claims are claims about a file on disk, not about a value in memory.

use arrow::array::{Array, ArrayRef, DictionaryArray, Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType as ArrowDataType, Int32Type, SchemaRef};
use arrow::record_batch::RecordBatch;
use columnar_testdata::generator::{DatasetGenerator, OrdersGenerator};
use columnar_testdata::manifest::{KeySpace, ORDERS_TABLE};
use columnar_testdata::schema::{self, ORDERS_COLUMNS, ORDERS_DICTIONARY_COLUMNS};
use columnar_testdata::writer::{write_orders_dataset, ROW_GROUP_ROWS};
use columnar_testdata::{orders_path_for, DatasetSpec};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

/// The rows used by the tests that do not need the whole table.
const TEST_ROWS: u64 = 100_000;

/// The key space of the `lineitem` table at SF-1.
const SF1_KEY_SPACE: u64 = 6_000_000;

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

/// The order keys of a file, in the order the file holds them.
fn order_keys(path: &Path) -> Vec<i64> {
    let mut keys = Vec::new();
    for batch in read_batches(path) {
        let array = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("o_orderkey is an int column");
        keys.extend(array.values().iter().copied());
    }
    keys
}

/// Writes an `orders` table of `rows` rows over a key space of `key_space`.
fn write(spec: &DatasetSpec, rows: u64, key_space: u64, name: &str) -> PathBuf {
    let path = test_path(name);
    write_orders_dataset(spec, rows, key_space, &path, ROW_GROUP_ROWS)
        .expect("the orders table is written");
    path
}

/// The batches of a `lineitem` table, generated in memory.
fn lineitem_batches(spec: &DatasetSpec, rows: u64, batch_rows: usize) -> Vec<RecordBatch> {
    let mut generator = DatasetGenerator::new(spec, rows).expect("the generator builds");
    let mut batches = Vec::new();
    let mut remaining = rows;
    while remaining > 0 {
        let size =
            usize::try_from(remaining.min(u64::try_from(batch_rows).expect("the size fits")))
                .expect("the size fits");
        batches.push(generator.next_batch(size).expect("the batch builds"));
        remaining -= u64::try_from(size).expect("the size fits");
    }
    batches
}

#[test]
fn the_same_seed_produces_byte_identical_orders_files() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x00C0_FFEE, null_rate: 0.1 };
    let first = write(&spec, TEST_ROWS, SF1_KEY_SPACE, "orders_determinism_a.parquet");
    let second = write(&spec, TEST_ROWS, SF1_KEY_SPACE, "orders_determinism_b.parquet");

    let first_bytes = std::fs::read(&first).expect("the first file reads");
    let second_bytes = std::fs::read(&second).expect("the second file reads");

    assert_eq!(first_bytes.len(), second_bytes.len(), "the two files have different sizes");
    assert!(first_bytes == second_bytes, "the two files differ byte for byte");
}

#[test]
fn a_different_seed_produces_different_orders_files() {
    let base = DatasetSpec { scale_factor: 1, seed: 1, null_rate: 0.0 };
    let other = DatasetSpec { seed: 2, ..base.clone() };
    let first = write(&base, 10_000, SF1_KEY_SPACE, "orders_seed_a.parquet");
    let second = write(&other, 10_000, SF1_KEY_SPACE, "orders_seed_b.parquet");

    assert_ne!(order_keys(&first), order_keys(&second), "the two tables have the same keys");
}

#[test]
fn the_order_keys_are_distinct() {
    let spec = DatasetSpec::default();
    let path = write(&spec, TEST_ROWS, SF1_KEY_SPACE, "orders_distinct.parquet");

    let keys = order_keys(&path);
    assert_eq!(keys.len() as u64, TEST_ROWS);

    let unique: HashSet<i64> = keys.iter().copied().collect();
    assert_eq!(unique.len(), keys.len(), "the order key is the table's primary key");
}

#[test]
fn every_order_key_is_inside_the_key_space() {
    let spec = DatasetSpec::default();
    let path = write(&spec, TEST_ROWS, SF1_KEY_SPACE, "orders_key_space.parquet");

    for key in order_keys(&path) {
        assert!(
            (1..=i64::try_from(SF1_KEY_SPACE).expect("the key space fits in an i64"))
                .contains(&key),
            "the order key {key} is outside the key space"
        );
    }
}

#[test]
fn the_orders_schema_has_the_nine_columns_in_order() {
    let spec = DatasetSpec::default();
    let path = write(&spec, 1_000, SF1_KEY_SPACE, "orders_schema.parquet");
    let schema = read_schema(&path);

    let expected_names: Vec<&str> = ORDERS_COLUMNS.iter().map(|column| column.name).collect();
    let actual_names: Vec<&str> =
        schema.fields().iter().map(|field| field.name().as_str()).collect();
    assert_eq!(actual_names, expected_names, "the column order is the contract");
    assert_eq!(ORDERS_COLUMNS.len(), 9);

    for (field, column) in schema.fields().iter().zip(ORDERS_COLUMNS.iter()) {
        let expected = schema::arrow_data_type(column);
        assert_eq!(field.data_type(), &expected, "column {} has the wrong type", column.name);
    }

    // Only the comment column is nullable.
    for field in schema.fields() {
        let nullable = field.name() == schema::ORDERS_COMMENT_COLUMN;
        assert_eq!(
            field.is_nullable(),
            nullable,
            "column {} has the wrong nullability",
            field.name()
        );
    }

    // The engine's vocabulary must agree with the Arrow schema.
    assert_eq!(schema::orders_engine_data_type("o_orderkey"), Some(columnar_core::DataType::Int64));
    assert_eq!(
        schema::orders_engine_data_type("o_totalprice"),
        Some(columnar_core::DataType::Float64)
    );
    assert_eq!(schema::orders_engine_data_type("o_orderdate"), Some(columnar_core::DataType::Utf8));
    assert_eq!(schema::orders_engine_data_type("o_nope"), None);
}

#[test]
fn the_low_cardinality_columns_are_dictionary_encoded() {
    let spec = DatasetSpec::default();
    let path = write(&spec, 10_000, SF1_KEY_SPACE, "orders_dictionary.parquet");
    let schema = read_schema(&path);

    for (index, column) in ORDERS_COLUMNS.iter().enumerate() {
        let field = schema.field(index);
        if column.dictionary {
            assert!(
                matches!(field.data_type(), ArrowDataType::Dictionary(_, _)),
                "column {} should be a dictionary in the Arrow schema",
                column.name
            );
            assert!(ORDERS_DICTIONARY_COLUMNS.contains(&column.name));
        } else {
            assert!(
                !matches!(field.data_type(), ArrowDataType::Dictionary(_, _)),
                "column {} should not be a dictionary",
                column.name
            );
        }
    }

    assert_eq!(ORDERS_DICTIONARY_COLUMNS, ["o_orderstatus", "o_orderdate", "o_orderpriority"]);
}

#[test]
fn generating_orders_does_not_change_the_lineitem_rows() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x5EED, null_rate: 0.1 };
    let rows = 50_000;
    let batch_rows = 8_192;

    // The `lineitem` rows on their own.
    let before = lineitem_batches(&spec, rows, batch_rows);

    // The `orders` table, in between.
    let mut orders =
        OrdersGenerator::new(&spec, 10_000, SF1_KEY_SPACE).expect("the generator builds");
    let orders_batch = orders.next_batch(10_000).expect("the batch builds");
    assert_eq!(orders_batch.num_rows(), 10_000);
    assert_eq!(orders_batch.num_columns(), ORDERS_COLUMNS.len());

    // The `lineitem` rows again, after the `orders` table has been made.
    let after = lineitem_batches(&spec, rows, batch_rows);

    assert_eq!(before.len(), after.len(), "the two runs made a different number of batches");
    for (index, (left, right)) in before.iter().zip(after.iter()).enumerate() {
        assert_eq!(left, right, "batch {index} of the lineitem table moved");
    }
}

#[test]
fn the_orders_row_count_follows_the_scale_factor() {
    assert_eq!(schema::orders_rows_for_scale_factor(1), 1_500_000);
    assert_eq!(schema::orders_rows_for_scale_factor(10), 15_000_000);
    assert_eq!(schema::orders_rows_for_scale_factor(0), 0);
    assert_eq!(
        schema::orders_rows_for_scale_factor(1) * 4,
        schema::rows_for_scale_factor(1),
        "orders is a quarter of lineitem"
    );
}

#[test]
fn the_manifest_names_the_key_space_and_the_match_rate() {
    let spec = DatasetSpec::default();
    let path = test_path("orders_manifest.parquet");
    let manifest = write_orders_dataset(&spec, TEST_ROWS, SF1_KEY_SPACE, &path, ROW_GROUP_ROWS)
        .expect("the orders table is written");

    assert_eq!(manifest.table, ORDERS_TABLE);
    assert_eq!(manifest.rows, TEST_ROWS);
    assert!(manifest.bytes > 0);
    assert_eq!(manifest.columns, schema::orders_column_names());

    let key_space = manifest.key_space.expect("the manifest names the key space");
    assert_eq!(key_space.probe_column, "l_orderkey");
    assert_eq!(key_space.build_column, "o_orderkey");
    assert_eq!(key_space.min, 1);
    assert_eq!(key_space.max, i64::try_from(SF1_KEY_SPACE).expect("the key space fits in an i64"));
    assert_eq!(key_space.distinct_keys, TEST_ROWS);
    assert!(
        (key_space.expected_match_rate - 100_000.0 / 6_000_000.0).abs() < 1e-12,
        "the match rate is {}",
        key_space.expected_match_rate
    );
}

#[test]
fn the_join_over_the_full_dataset_matches_a_quarter_of_the_rows() {
    // At SF-1 the `orders` table holds 1,500,000 distinct keys over a
    // 6,000,000 key space, so a quarter of `lineitem`'s rows find a
    // partner. This is the number the published join figure depends on.
    let key_space =
        KeySpace::join(schema::rows_for_scale_factor(1), schema::orders_rows_for_scale_factor(1));

    assert_eq!(key_space.max, 6_000_000);
    assert_eq!(key_space.distinct_keys, 1_500_000);
    assert!(
        (key_space.expected_match_rate - 0.25).abs() < 1e-12,
        "the match rate is {}",
        key_space.expected_match_rate
    );
}

#[test]
fn the_null_rate_is_injected_into_the_comment_column() {
    let spec = DatasetSpec { scale_factor: 1, seed: 0x5EED, null_rate: 0.1 };
    let path = write(&spec, TEST_ROWS, SF1_KEY_SPACE, "orders_nulls.parquet");

    let mut nulls = 0u64;
    let mut total = 0u64;
    for batch in read_batches(&path) {
        let array = batch
            .column(8)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("o_comment is a string column");
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
fn the_values_are_in_range() {
    let spec = DatasetSpec::default();
    let path = write(&spec, 50_000, SF1_KEY_SPACE, "orders_ranges.parquet");

    for batch in read_batches(&path) {
        let custkey = batch
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("o_custkey is an int column");
        for value in custkey.values() {
            assert!((1..=150_000).contains(value), "o_custkey is {value}");
        }

        let totalprice = batch
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("o_totalprice is a float column");
        for value in totalprice.values() {
            assert!((900.0..=105_000.0).contains(value), "o_totalprice is {value}");
            // Every value has exactly two decimal places.
            let cents = value * 100.0;
            assert!((cents - cents.round()).abs() < 1e-6, "o_totalprice is {value}");
        }

        let clerk = batch
            .column(6)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("o_clerk is an int column");
        for value in clerk.values() {
            assert!((1..=1_000).contains(value), "o_clerk is {value}");
        }

        let shippriority = batch
            .column(7)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("o_shippriority is an int column");
        for value in shippriority.values() {
            assert!((0..=1).contains(value), "o_shippriority is {value}");
        }

        for status in dictionary_strings(batch.column(2)) {
            let status = status.expect("o_orderstatus is not null");
            assert!(["O", "F", "P"].contains(&status.as_str()), "o_orderstatus is {status}");
        }

        for priority in dictionary_strings(batch.column(5)) {
            let priority = priority.expect("o_orderpriority is not null");
            assert!(
                ["1-URGENT", "2-HIGH", "3-MEDIUM", "4-NOT SPECIFIED", "5-LOW"]
                    .contains(&priority.as_str()),
                "o_orderpriority is {priority}"
            );
        }
    }
}

#[test]
fn the_order_dates_cover_the_shipping_window() {
    let spec = DatasetSpec::default();
    let path = write(&spec, 10_000, SF1_KEY_SPACE, "orders_dates.parquet");

    let start = columnar_testdata::generator::days_from_civil(1992, 1, 2);
    let end = columnar_testdata::generator::days_from_civil(1998, 12, 1);

    for batch in read_batches(&path) {
        for date in dictionary_strings(batch.column(4)) {
            let date = date.expect("o_orderdate is not null");
            let (year, month, day) = (
                date.get(0..4).expect("the year is there").parse().expect("the year parses"),
                date.get(5..7).expect("the month is there").parse().expect("the month parses"),
                date.get(8..10).expect("the day is there").parse().expect("the day parses"),
            );
            let number = columnar_testdata::generator::days_from_civil(year, month, day);
            assert!((start..=end).contains(&number), "o_orderdate is {date}");
        }
    }
}

#[test]
fn the_orders_file_sits_beside_the_lineitem_file() {
    assert_eq!(
        orders_path_for(Path::new("data/lineitem.parquet")),
        PathBuf::from("data/orders.parquet")
    );
    assert_eq!(
        orders_path_for(Path::new("data/lineitem-rg24.parquet")),
        PathBuf::from("data/orders-rg24.parquet")
    );
    assert_eq!(orders_path_for(Path::new("lineitem.parquet")), PathBuf::from("orders.parquet"));
}

#[test]
fn a_table_that_needs_more_keys_than_the_space_holds_is_rejected() {
    let spec = DatasetSpec::default();
    let path = test_path("orders_impossible.parquet");
    let error = write_orders_dataset(&spec, 1_000, 100, &path, ROW_GROUP_ROWS)
        .expect_err("the table is rejected");

    assert!(
        matches!(error, columnar_testdata::DatasetError::InvalidSpec(_)),
        "the error is {error:?}"
    );
}
