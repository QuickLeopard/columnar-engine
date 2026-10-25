//! The Parquet round trip.
//!
//! The test writes a small `lineitem`-shaped Parquet file with the same
//! schema the TPC-H generator produces -- the eight numeric columns, the
//! seven dictionary-encoded string columns and the free-text comment column
//! -- reads it back through [`Dataset::read_parquet`], and checks both the
//! data and the encodings.
//!
//! The file is written by the test itself: a binary fixture in the
//! repository would be a blob nobody could review, and a generator that
//! nobody could run would be worse.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{ArrayRef, DictionaryArray, Float64Array, Int32Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Encoding, Predicate, ScalarValue};
use columnar_engine::{
    execute, Aggregate, AggregateSpec, AggregateValue, Dataset, ExecOptions, MemoryBudget,
    QueryPlan,
};
use columnar_kernels::Flavor;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::schema::types::ColumnPath;

/// The number of rows the generated file holds.
const ROWS: usize = 10_000;

/// The names of the seven dictionary-encoded string columns.
const DICTIONARY_COLUMNS: [&str; 7] = [
    "l_returnflag",
    "l_linestatus",
    "l_shipdate",
    "l_commitdate",
    "l_receiptdate",
    "l_shipinstruct",
    "l_shipmode",
];

/// A dictionary-encoded UTF-8 column.
fn dictionary_column(keys: Vec<i32>, values: Vec<&str>) -> ArrayRef {
    let values: ArrayRef = Arc::new(StringArray::from(values));
    Arc::new(DictionaryArray::new(Int32Array::from(keys), values))
}

/// The `lineitem` schema the TPC-H generator produces.
fn lineitem_schema() -> Arc<Schema> {
    let mut fields: Vec<Field> = vec![
        Field::new("l_orderkey", DataType::Int64, true),
        Field::new("l_partkey", DataType::Int64, true),
        Field::new("l_suppkey", DataType::Int64, true),
        Field::new("l_linenumber", DataType::Int64, true),
        Field::new("l_quantity", DataType::Float64, true),
        Field::new("l_extendedprice", DataType::Float64, true),
        Field::new("l_discount", DataType::Float64, true),
        Field::new("l_tax", DataType::Float64, true),
    ];
    for name in DICTIONARY_COLUMNS {
        fields.push(Field::new(
            name,
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ));
    }
    fields.push(Field::new("l_comment", DataType::Utf8, true));
    Arc::new(Schema::new(fields))
}

/// Builds the `lineitem` rows.
///
/// `l_extendedprice` is a multiple of `0.5`, so every sum over it is exact
/// in a double and the expected value can be computed without a tolerance.
fn lineitem_batch() -> RecordBatch {
    let orderkey: Vec<i64> =
        (0..ROWS).map(|row| i64::try_from(row).expect("a row index fits in an i64")).collect();
    let partkey: Vec<i64> = (0..ROWS)
        .map(|row| i64::try_from(row % 1000).expect("a small row index fits in an i64"))
        .collect();
    let suppkey: Vec<i64> = (0..ROWS)
        .map(|row| i64::try_from(row % 5000).expect("a small row index fits in an i64"))
        .collect();
    let linenumber: Vec<i64> = (0..ROWS)
        .map(|row| i64::try_from(row % 7 + 1).expect("a small row index fits in an i64"))
        .collect();
    let quantity: Vec<f64> = (0..ROWS)
        .map(|row| f64::from(u32::try_from(row % 50).expect("a small index fits in a u32")) + 0.5)
        .collect();
    let extendedprice: Vec<f64> = (0..ROWS)
        .map(|row| f64::from(u32::try_from(row).expect("a row index fits in a u32")) * 0.5)
        .collect();
    let discount: Vec<f64> = (0..ROWS)
        .map(|row| f64::from(u32::try_from(row % 11).expect("a small index fits in a u32")) * 0.01)
        .collect();
    let tax: Vec<f64> = vec![0.08; ROWS];

    let flags = ["N", "A"];
    let statuses = ["O", "F"];
    let shipdates = ["1996-01-02", "1996-02-03"];
    let instructions = ["DELIVER IN PERSON", "COLLECT COD"];
    let modes = ["AIR", "RAIL"];

    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(orderkey)),
        Arc::new(Int64Array::from(partkey)),
        Arc::new(Int64Array::from(suppkey)),
        Arc::new(Int64Array::from(linenumber)),
        Arc::new(Float64Array::from(quantity)),
        Arc::new(Float64Array::from(extendedprice)),
        Arc::new(Float64Array::from(discount)),
        Arc::new(Float64Array::from(tax)),
    ];

    // Every seventh column gets its own small dictionary.
    for (index, values) in
        [flags, statuses, shipdates, shipdates, shipdates, instructions, modes].iter().enumerate()
    {
        let keys: Vec<i32> = (0..ROWS)
            .map(|row| {
                i32::try_from((row + index) % values.len()).expect("a small key fits in an i32")
            })
            .collect();
        columns.push(dictionary_column(keys, values.to_vec()));
    }

    // Every hundredth comment is NULL, so the null path is exercised too.
    let comments: ArrayRef = Arc::new(StringArray::from(
        (0..ROWS)
            .map(|row| if row % 100 == 0 { None } else { Some(format!("comment for order {row}")) })
            .collect::<Vec<Option<String>>>(),
    ));
    columns.push(comments);

    RecordBatch::try_new(lineitem_schema(), columns).expect("the batch matches the schema")
}

/// Writes the `lineitem` file and reads it back.
fn round_trip() -> (Dataset, PathBuf) {
    let path = std::env::temp_dir().join("columnar-engine-lineitem-roundtrip.parquet");
    let file = File::create(&path).expect("the temporary file can be created");
    let properties = WriterProperties::builder()
        // The comment column is high-cardinality: it is stored plainly, so
        // the reader has to cope with a plain string column as well.
        .set_column_dictionary_enabled(ColumnPath::from("l_comment"), false)
        .build();
    let mut writer =
        ArrowWriter::try_new(file, lineitem_schema(), Some(properties)).expect("the writer starts");
    writer.write(&lineitem_batch()).expect("the batch is written");
    writer.close().expect("the file is closed");

    let dataset = Dataset::read_parquet(&path).expect("the file is read back");
    (dataset, path)
}

#[test]
fn the_round_trip_keeps_the_rows_and_the_schema() {
    let (dataset, path) = round_trip();

    assert_eq!(dataset.len(), ROWS);
    assert!(!dataset.is_empty());

    let mut expected_names = vec![
        "l_orderkey",
        "l_partkey",
        "l_suppkey",
        "l_linenumber",
        "l_quantity",
        "l_extendedprice",
        "l_discount",
        "l_tax",
    ];
    expected_names.extend(DICTIONARY_COLUMNS);
    expected_names.push("l_comment");
    assert_eq!(dataset.column_names(), expected_names);

    // The numeric columns keep their values.
    let orderkey = dataset.chunk("l_orderkey").expect("the column is there");
    assert_eq!(orderkey.value_at(0), Some(ScalarValue::Int64(0)));
    assert_eq!(
        orderkey.value_at(ROWS - 1),
        Some(ScalarValue::Int64(i64::try_from(ROWS - 1).expect("a row index fits in an i64")))
    );
    let price = dataset.chunk("l_extendedprice").expect("the column is there");
    assert_eq!(
        price.value_at(3),
        Some(ScalarValue::Float64(1.5)),
        "the price is a multiple of 0.5"
    );

    // The dictionary columns keep their dictionary.
    for name in DICTIONARY_COLUMNS {
        let chunk = dataset.chunk(name).expect("the column is there");
        assert!(
            matches!(chunk.encoding, Encoding::Utf8Dict { .. }),
            "column `{name}` did not keep its dictionary"
        );
    }

    // The comment column is plain, and its NULLs survived.
    let comment = dataset.chunk("l_comment").expect("the column is there");
    assert!(
        matches!(comment.encoding, Encoding::Utf8 { .. }),
        "the comment column is not a plain string column"
    );
    assert_eq!(comment.validity.null_count(), ROWS / 100);
    assert_eq!(comment.value_at(0), None);
    assert_eq!(comment.value_at(1), Some(ScalarValue::Utf8("comment for order 1".to_owned())));

    let _ = std::fs::remove_file(path);
}

#[test]
fn the_round_trip_answers_a_query() {
    let (dataset, path) = round_trip();

    // `l_discount < 0.05` selects the rows whose `row % 11` is 0..=4.
    let plan = QueryPlan {
        filter: vec![Predicate::new("l_discount", ComparisonOp::Lt, 0.05f64)],
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec!["l_returnflag".to_owned()],
            aggregates: vec![Aggregate::Sum { column: "l_extendedprice".to_owned() }],
        }),
    };

    let options = ExecOptions {
        memory_budget: MemoryBudget::default(),
        flavor: Some(Flavor::Scalar),
        ..ExecOptions::default()
    };
    let result = execute(&plan, &dataset, &options).expect("the plan runs");

    // The expected sums, computed from the same rule the data was built
    // with: `l_returnflag` is "N" on even rows and "A" on odd ones, and
    // `l_extendedprice` is a multiple of 0.5, so every sum is exact in a
    // double and the comparison is bit-for-bit.
    let mut expected: HashMap<&str, f64> = HashMap::new();
    for row in 0..ROWS {
        if row % 11 < 5 {
            let flag = if row % 2 == 0 { "N" } else { "A" };
            *expected.entry(flag).or_insert(0.0) +=
                f64::from(u32::try_from(row).expect("a row index fits in a u32")) * 0.5;
        }
    }

    assert_eq!(result.groups.len(), expected.len(), "two return flags survive");
    for group in &result.groups {
        let Some(ScalarValue::Utf8(flag)) = group.key.first() else {
            panic!("unexpected group key {:?}", group.key);
        };
        let expected_sum =
            expected.get(flag.as_str()).unwrap_or_else(|| panic!("unexpected group `{flag}`"));
        match group.values.first() {
            Some(AggregateValue::Float(actual)) => assert_eq!(
                actual.to_bits(),
                expected_sum.to_bits(),
                "the sum for `{flag}` differs: {actual} != {expected_sum}"
            ),
            other => panic!("unexpected aggregate value {other:?}"),
        }
    }

    let _ = std::fs::remove_file(path);
}
