//! The join's build side, over a set of files.
//!
//! # What this file is
//!
//! The join's build side is the right-hand table: the one the hash table is built from before the
//! probe looks anything up. It was named by **one path**, and spec 042 gave it **a set**. This file
//! says three things:
//!
//! - **A set is the same table.** The build side built from one file and the build side built from
//!   two files give the **same rows and the same answer** -- so a store's table, which is several
//!   files, can be the build side of a join.
//! - **One path is still one path.** The plan names a list of paths, and a list of one is opened,
//!   read and charged as the one file it was.
//! - **An empty list is refused.** A build side that names no file is a plan that cannot run, and
//!   it says so before anything is read.
//!
//! The engine's own rows are the other half of the same increment, and they are in the store's test
//! suite (`crates/columnar-store/tests/engine_rows.rs`): the store's rows are the engine's rows, and
//! the engine's rows are the store's rows.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Int64Array, RecordBatch};
use arrow::datatypes::{DataType, Field, Schema};
use columnar_engine::{
    execute_on, execute_rows, open_source, Aggregate, AggregateSpec, AggregateValue, Dataset,
    ExecOptions, Expr, JoinSpec, QueryPlan,
};
use parquet::arrow::ArrowWriter;

/// A scratch directory for the test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("columnar-engine-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The schema of the test's tables.
///
/// The two sides have **different** names for their second column, so the
/// join's output says which side a value came from.
fn schema(second: &str) -> Schema {
    Schema::new(vec![
        Field::new("k", DataType::Int64, false),
        Field::new(second, DataType::Int64, false),
    ])
}

/// Writes a table of `keys` and `values`, and returns the path.
fn write_table(path: &Path, keys: &[i64], values: &[i64], second: &str) -> PathBuf {
    let batch = RecordBatch::try_new(
        Arc::new(schema(second)),
        vec![
            Arc::new(Int64Array::from(keys.to_vec())),
            Arc::new(Int64Array::from(values.to_vec())),
        ],
    )
    .expect("the batch is made");
    let file = File::create(path).expect("the file is created");
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).expect("the writer is built");
    writer.write(&batch).expect("the rows are written");
    writer.close().expect("the file closes");
    path.to_path_buf()
}

/// The build side's two files, and the probe side's file.
///
/// The directory is **per test**, so the tests do not remove each other's
/// files while they run.
fn tables(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = scratch(name);
    std::fs::create_dir_all(&dir).expect("the directory is created");
    let first =
        write_table(&dir.join("first.parquet"), &[1, 2, 3, 4, 5], &[10, 20, 30, 40, 50], "v");
    let second =
        write_table(&dir.join("second.parquet"), &[6, 7, 8, 9, 10], &[60, 70, 80, 90, 100], "v");
    let probe = write_table(
        &dir.join("probe.parquet"),
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        "w",
    );
    (first, second, probe)
}

/// The plan: the probe side's file joined to a build side.
///
/// The aggregate is over the **probe side's** column, because the join's output
/// is the probe side's rows -- the build side's columns are not carried into it.
fn plan(build_paths: Vec<PathBuf>) -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("w") }],
        }),
        join: Some(JoinSpec {
            build_paths,
            probe_key: "k".to_owned(),
            build_key: "k".to_owned(),
            build_filter: Vec::new(),
        }),
        sort: None,
    }
}

/// The sum the join produced.
fn answer(result: &columnar_engine::AggregateResult) -> i64 {
    let group = result.groups.first().expect("the group is there");
    match group.values.first().expect("the value is there") {
        AggregateValue::Int(value) => *value,
        other => panic!("the sum is an integer, not {other:?}"),
    }
}

#[test]
fn a_set_is_the_same_table() {
    let (first, second, probe) = tables("set");
    let source = open_source(&probe).expect("the probe side opens");

    let one = execute_on(&plan(vec![first.clone()]), &source, &ExecOptions::default())
        .expect("the join runs");
    let set =
        execute_on(&plan(vec![first.clone(), second.clone()]), &source, &ExecOptions::default())
            .expect("the join runs");

    assert_eq!(answer(&one), 15, "one file's rows are the rows that match");
    assert_eq!(answer(&set), 55, "a set's rows are the rows of both files");
    assert_eq!(
        one.plan_output.join.expect("the plan had a join").output_rows,
        5,
        "one file's rows match the probe's five rows"
    );
    assert_eq!(
        set.plan_output.join.expect("the plan had a join").output_rows,
        10,
        "a set's rows match the probe's ten rows"
    );
}

#[test]
fn one_path_is_one_path() {
    let (first, _second, probe) = tables("one");
    let source = open_source(&probe).expect("the probe side opens");
    let result =
        execute_on(&plan(vec![first]), &source, &ExecOptions::default()).expect("the join runs");
    assert_eq!(answer(&result), 15, "a list of one is the one file it names");
}

#[test]
fn an_empty_build_side_is_refused() {
    let (_first, _second, probe) = tables("empty");
    let source = open_source(&probe).expect("the probe side opens");
    let error = execute_on(&plan(Vec::new()), &source, &ExecOptions::default())
        .expect_err("a build side that names no file is refused");
    assert!(error.to_string().contains("build side"), "the error names the build side: {error}");
}

#[test]
fn a_plans_rows_are_its_projection() {
    let dataset = Dataset::from_chunks(vec![
        columnar_engine::ColumnChunk {
            name: "k".to_owned(),
            encoding: columnar_engine::Encoding::Int64 { values: vec![1, 2, 3, 4] },
            validity: columnar_engine::NullBitmap::all_valid(4),
            len: 4,
        },
        columnar_engine::ColumnChunk {
            name: "v".to_owned(),
            encoding: columnar_engine::Encoding::Int64 { values: vec![10, 20, 30, 40] },
            validity: columnar_engine::NullBitmap::all_valid(4),
            len: 4,
        },
    ]);

    let query = QueryPlan {
        filter: vec![columnar_engine::Predicate::new(
            "k",
            columnar_engine::ComparisonOp::Gt,
            columnar_engine::ScalarValue::Int64(2),
        )],
        projection: vec!["v".to_owned()],
        aggregate: None,
        join: None,
        sort: None,
    };
    let rows = execute_rows(&query, &dataset, &ExecOptions::default()).expect("the rows come out");
    assert_eq!(rows.rows, 2, "the filter's survivors are the rows");
    assert_eq!(rows.chunks.len(), 1, "the projection's columns are the columns");
    assert_eq!(rows.chunks.first().expect("the column is there").name, "v");
}

#[test]
fn a_plan_with_an_aggregate_has_no_rows_to_write() {
    let dataset = Dataset::from_chunks(vec![columnar_engine::ColumnChunk {
        name: "k".to_owned(),
        encoding: columnar_engine::Encoding::Int64 { values: vec![1, 2, 3, 4] },
        validity: columnar_engine::NullBitmap::all_valid(4),
        len: 4,
    }]);
    let query = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("k") }],
        }),
        join: None,
        sort: None,
    };
    let error = execute_rows(&query, &dataset, &ExecOptions::default())
        .expect_err("a plan with an aggregate has no rows");
    assert!(error.to_string().contains("no rows to write"), "the error says why: {error}");
}
