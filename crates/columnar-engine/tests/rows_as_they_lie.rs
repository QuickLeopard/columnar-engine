//! The rows as they lie.
//!
//! # What this file is
//!
//! The rows path used to **gather** every column it handed out: the plan's
//! projection, copied out of what was read, with the dictionary materialised
//! away. A plan that hands out exactly the rows that were read paid for that
//! copy and got nothing for it.
//!
//! So the rows are handed out **as they lie** when two things hold:
//!
//! - **No row was removed.** The filter's selection holds the survivors in
//!   the order they were read, so a selection as long as the dataset *is*
//!   every row.
//! - **Nothing reordered them.** A join's output and a sort's output are new
//!   rows, in a new order.
//!
//! **The test is about the rows, not about where they came from.** The eager
//! path and the lazy path make the same one, and they can answer it
//! differently -- because the *read* is different. The lazy path pushes the
//! plan's predicates into the reader, so a predicate the reader took has
//! already removed its rows and the rows that come out are the ones that were
//! read. The eager path reads everything and filters in the engine, so the
//! survivors are scattered through the dataset and they are gathered.
//!
//! # What is checked here
//!
//! - **The rows as they lie.** A plan that removes nothing hands out the
//!   columns with the reader's own encodings: the dictionary is there.
//! - **The rows as they were.** A plan that removes rows in the engine, a
//!   join and a sort all gather the rows, exactly as they always did.
//! - **One rule, both paths.** The same plan, over the same file, through
//!   both paths, hands out the same rows.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, DictionaryArray, Int32Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Encoding, Predicate, ScalarValue};
use columnar_engine::{
    execute_rows, execute_rows_on, open_source, Dataset, ExecOptions, JoinSpec, QueryPlan, SortSpec,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

/// The number of rows the generated file holds.
const ROWS: usize = 4_096;

/// A dictionary-encoded UTF-8 column.
fn dictionary_column(keys: Vec<i32>, values: Vec<&str>) -> ArrayRef {
    let values: ArrayRef = Arc::new(StringArray::from(values));
    Arc::new(DictionaryArray::new(Int32Array::from(keys), values))
}

/// The schema: a key, a dictionary column, and a column the plan can filter on.
fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("k", DataType::Int64, false),
        Field::new(
            "v",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            false,
        ),
        Field::new("n", DataType::Int64, false),
    ]))
}

/// The rows: a key, a dictionary column with three values, and a number.
fn batch() -> RecordBatch {
    let keys: Vec<i64> =
        (0..ROWS).map(|row| i64::try_from(row).expect("a row index fits in an i64")).collect();
    let dictionary_keys: Vec<i32> = (0..ROWS)
        .map(|row| i32::try_from(row % 3).expect("a small index fits in an i32"))
        .collect();
    let numbers: Vec<i64> = (0..ROWS)
        .map(|row| i64::try_from(row % 7).expect("a small row index fits in an i64"))
        .collect();
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from(keys)),
            dictionary_column(dictionary_keys, vec!["alpha", "beta", "gamma"]),
            Arc::new(Int64Array::from(numbers)),
        ],
    )
    .expect("the batch is built")
}

/// Writes the file the tests read.
fn write_file(path: &Path) {
    let file = File::create(path).expect("the file is created");
    let properties = WriterProperties::builder().build();
    let mut writer =
        ArrowWriter::try_new(file, schema(), Some(properties)).expect("the writer is built");
    writer.write(&batch()).expect("the rows are written");
    writer.close().expect("the file is closed");
}

/// A file in a scratch directory of this test's own.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("columnar-rows-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the scratch directory is made");
    dir.join("rows.parquet")
}

/// The encoding of the column named `name`, in the rows.
fn encoding_of(rows: &Dataset, name: &str) -> Encoding {
    let chunk = rows.chunks.iter().find(|chunk| chunk.name == name).expect("the column is there");
    chunk.encoding.clone()
}

/// The plan with no filter and no projection: every column, every row.
fn plan() -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: None,
        join: None,
        sort: None,
    }
}

#[test]
fn a_plan_that_removes_nothing_hands_out_the_rows_as_they_lie() {
    let path = scratch("lie");
    write_file(&path);

    // The eager path: the file is read, and the rows are handed out as they
    // were read.
    let dataset = Dataset::read_parquet(&path).expect("the file reads");
    let eager =
        execute_rows(&plan(), &dataset, &ExecOptions::default()).expect("the rows come out");
    assert!(
        matches!(encoding_of(&eager, "v"), Encoding::Utf8Dict { .. }),
        "the eager path's rows came out plain: the dictionary was materialised away"
    );

    // The lazy path: the same file, read by the reader, and the rows are
    // handed out as they were read.
    let source = open_source(&path).expect("the source opens");
    let lazy =
        execute_rows_on(&plan(), &source, &ExecOptions::default()).expect("the rows come out");
    assert!(
        matches!(encoding_of(&lazy, "v"), Encoding::Utf8Dict { .. }),
        "the lazy path's rows came out plain: the dictionary was materialised away"
    );

    // And the two paths hand out the same rows.
    assert_eq!(eager.chunks, lazy.chunks, "the two paths handed out different rows");
    assert_eq!(eager.rows, lazy.rows, "the two paths handed out a different number of rows");
}

#[test]
fn a_plan_that_filters_in_the_engine_hands_out_the_gathered_rows() {
    let path = scratch("filtered");
    write_file(&path);

    // The eager path reads everything and filters in the engine, so the
    // survivors are scattered through the dataset and the rows are gathered.
    let dataset = Dataset::read_parquet(&path).expect("the file reads");
    let filtered = QueryPlan {
        filter: vec![Predicate::new("k", ComparisonOp::GtEq, ScalarValue::Int64(100))],
        ..plan()
    };
    let eager =
        execute_rows(&filtered, &dataset, &ExecOptions::default()).expect("the rows come out");
    assert_eq!(eager.rows, ROWS - 100, "the filter did not remove the rows it should");
    assert!(
        matches!(encoding_of(&eager, "v"), Encoding::Utf8 { .. }),
        "the filtered rows came out as they lie: the survivors are not the rows that were read"
    );
}

#[test]
fn a_plan_the_reader_filters_hands_out_the_rows_as_they_lie() {
    let path = scratch("pushed");
    write_file(&path);

    // The lazy path pushes the predicate into the reader, and the reader
    // removes the rows that fail it. **So the rows that come out are the rows
    // that were read**, and they are handed out as they lie.
    let source = open_source(&path).expect("the source opens");
    let filtered = QueryPlan {
        filter: vec![Predicate::new("n", ComparisonOp::Lt, ScalarValue::Int64(3))],
        // The filter's column is not read: the predicate is on `n`, and the
        // plan materialises `k` and `v`.
        projection: vec!["k".to_owned(), "v".to_owned()],
        ..plan()
    };
    let rows =
        execute_rows_on(&filtered, &source, &ExecOptions::default()).expect("the rows come out");
    // 4096 rows, `n` is the row index modulo seven, and `n < 3` keeps three
    // of every seven -- 1755 from the 585 whole groups, and one more from the
    // row that is left over.
    assert_eq!(rows.rows, 1_756, "the reader did not remove the rows it should");
    assert!(
        matches!(encoding_of(&rows, "v"), Encoding::Utf8Dict { .. }),
        "the rows the reader filtered came out plain: the dictionary was materialised away"
    );
}

#[test]
fn a_sort_hands_out_the_gathered_rows() {
    let path = scratch("sorted");
    write_file(&path);

    // A sort makes new rows in a new order, so the rows are gathered.
    let sorted = QueryPlan {
        sort: Some(SortSpec { column: "k".to_owned(), descending: true, limit: None }),
        ..plan()
    };
    let dataset = Dataset::read_parquet(&path).expect("the file reads");
    let rows = execute_rows(&sorted, &dataset, &ExecOptions::default()).expect("the rows come out");
    assert!(
        matches!(encoding_of(&rows, "v"), Encoding::Utf8 { .. }),
        "the sorted rows came out as they lie: the sort made new rows"
    );
}

#[test]
fn a_join_hands_out_the_gathered_rows() {
    let path = scratch("joined");
    write_file(&path);
    let build = scratch("joined-build");
    write_file(&build);

    // A join makes new rows, so the rows are gathered.
    let joined = QueryPlan {
        join: Some(JoinSpec {
            build_paths: vec![build],
            probe_key: "k".to_owned(),
            build_key: "k".to_owned(),
            build_filter: Vec::new(),
        }),
        ..plan()
    };
    let dataset = Dataset::read_parquet(&path).expect("the file reads");
    let rows = execute_rows(&joined, &dataset, &ExecOptions::default()).expect("the rows come out");
    assert_eq!(rows.rows, ROWS, "the join dropped the rows it should keep");
    assert!(
        matches!(encoding_of(&rows, "v"), Encoding::Utf8 { .. }),
        "the joined rows came out as they lie: the join made new rows"
    );
}
