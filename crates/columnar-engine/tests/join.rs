//! The hash join, against a brute-force nested loop.
//!
//! The reference here is the slowest possible join: every probe row against
//! every build row, no hashing, no index, nothing clever. The hash join has
//! to agree with it **row for row and in order** -- the same rows, the same
//! order, the same values -- because a join that produces the same *set* of
//! rows in a different order is a different answer whenever the rows are
//! summed.
//!
//! The build side is a Parquet file, because that is what the plan holds: the
//! build side is the right table, and the engine reads it through its own
//! source. The probe side is the left table, and it is tested both in memory
//! and as a file.

// The indices below are literals into vectors whose length the test has just
// asserted, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Encoding, Expr, NullBitmap, Predicate, ScalarValue};
use columnar_engine::{
    execute, execute_on, Aggregate, AggregateSpec, AggregateValue, Dataset, ExecOptions, JoinSpec,
    MemoryBudget, ParquetSource, PlanOutput, QueryPlan, StageStats,
};
use columnar_kernels::Flavor;
use parquet::arrow::ArrowWriter;

/// The scalar options, so a test never depends on what the machine can run.
fn scalar() -> ExecOptions {
    ExecOptions {
        memory_budget: MemoryBudget::default(),
        flavor: Some(Flavor::Scalar),
        ..ExecOptions::default()
    }
}

/// A column chunk with the given NULL rows.
fn chunk(name: &str, encoding: Encoding, nulls: &[usize]) -> columnar_core::ColumnChunk {
    let len = encoding.len();
    columnar_core::ColumnChunk::new(name, encoding, NullBitmap::with_nulls(len, nulls))
}

/// The probe side, as a dataset.
///
/// The probe side is the left table, and the engine never materialises it as
/// a table of its own: the plan's predicates are pushed into its scan and
/// the rows that fail them never reach the join.
fn probe_dataset(keys: &[Option<i64>], values: &[Option<f64>]) -> Dataset {
    let key_nulls = null_rows(keys);
    let value_nulls = null_rows(values);
    Dataset::from_chunks(vec![
        chunk(
            "l_orderkey",
            Encoding::Int64 { values: keys.iter().map(|key| key.unwrap_or_default()).collect() },
            &key_nulls,
        ),
        chunk(
            "l_extendedprice",
            Encoding::Float64 {
                values: values.iter().map(|value| value.unwrap_or_default()).collect(),
            },
            &value_nulls,
        ),
    ])
}

/// The rows of a column that hold no value.
fn null_rows<T>(values: &[Option<T>]) -> Vec<usize> {
    values.iter().enumerate().filter_map(|(row, value)| value.is_none().then_some(row)).collect()
}

/// Writes the build side's file: an order key and an order date.
fn write_build(path: &Path, keys: &[Option<i64>], dates: &[Option<&str>]) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("o_orderkey", DataType::Int64, true),
        Field::new("o_orderdate", DataType::Utf8, true),
    ]));
    let keys: ArrayRef = Arc::new(Int64Array::from(keys.to_vec()));
    let dates: ArrayRef = Arc::new(StringArray::from(
        dates.iter().map(|date| date.map(str::to_owned)).collect::<Vec<Option<String>>>(),
    ));
    let batch = RecordBatch::try_new(schema.clone(), vec![keys, dates])
        .expect("the batch matches the schema");
    let file = File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer starts");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is closed");
}

/// Writes the probe side's file, so the lazy path can be tested too.
fn write_probe(path: &Path, keys: &[Option<i64>], values: &[Option<f64>]) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("l_orderkey", DataType::Int64, true),
        Field::new("l_extendedprice", DataType::Float64, true),
    ]));
    let keys: ArrayRef = Arc::new(Int64Array::from(keys.to_vec()));
    let values: ArrayRef = Arc::new(arrow::array::Float64Array::from(values.to_vec()));
    let batch = RecordBatch::try_new(schema.clone(), vec![keys, values])
        .expect("the batch matches the schema");
    let file = File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer starts");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is closed");
}

/// A path for a test's temporary file.
fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("columnar-engine-join-{name}.parquet"))
}

/// The join's plan.
fn join_plan(build_path: &Path, build_filter: Vec<Predicate>) -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![
                Aggregate::CountStar,
                Aggregate::Sum { expr: Expr::column("l_extendedprice") },
            ],
        }),
        join: Some(JoinSpec {
            build_path: build_path.to_path_buf(),
            probe_key: "l_orderkey".to_owned(),
            build_key: "o_orderkey".to_owned(),
            build_filter,
        }),
        sort: None,
    }
}

/// The joined rows, by brute force.
///
/// Every probe row against every build row, no hashing: the answer the hash
/// join has to agree with. A NULL key matches nothing, on either side, which
/// is SQL's rule and the engine's rule everywhere else.
fn nested_loop(probe: &[Option<i64>], build: &[Option<i64>]) -> Vec<usize> {
    let mut out = Vec::new();
    for (row, probe_key) in probe.iter().enumerate() {
        for build_key in build {
            if probe_key.is_some() && build_key.is_some() && probe_key == build_key {
                out.push(row);
            }
        }
    }
    out
}

/// The sum of the joined rows, in the order they were joined.
fn reference_sum(values: &[Option<f64>], rows: &[usize]) -> Option<f64> {
    let mut sum = 0.0;
    let mut seen = false;
    for &row in rows {
        if let Some(value) = values[row] {
            sum += value;
            seen = true;
        }
    }
    seen.then_some(sum)
}

/// The engine's answer: how many rows the join produced, and their sum.
fn engine_answer(plan: &QueryPlan, dataset: &Dataset) -> (i64, Option<f64>) {
    let result = execute(plan, dataset, &scalar()).expect("the plan runs");
    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    let sum = match group.values.get(1) {
        Some(AggregateValue::Float(sum)) => Some(*sum),
        Some(AggregateValue::Null) => None,
        other => panic!("unexpected sum {other:?}"),
    };
    (count, sum)
}

/// The engine's answer, over a Parquet file rather than a dataset.
fn engine_answer_on(plan: &QueryPlan, path: &Path) -> (i64, Option<f64>) {
    let source = ParquetSource::open(path).expect("the source opens");
    let result = execute_on(plan, &source, &scalar()).expect("the plan runs");
    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    let sum = match group.values.get(1) {
        Some(AggregateValue::Float(sum)) => Some(*sum),
        Some(AggregateValue::Null) => None,
        other => panic!("unexpected sum {other:?}"),
    };
    (count, sum)
}

/// The engine's answer and its plan output, over a Parquet file.
fn engine_answer_and_output(plan: &QueryPlan, path: &Path) -> ((i64, Option<f64>), PlanOutput) {
    let source = ParquetSource::open(path).expect("the source opens");
    let result = execute_on(plan, &source, &scalar()).expect("the plan runs");
    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    let sum = match group.values.get(1) {
        Some(AggregateValue::Float(sum)) => Some(*sum),
        Some(AggregateValue::Null) => None,
        other => panic!("unexpected sum {other:?}"),
    };
    ((count, sum), result.plan_output)
}

/// The join's own stage, in a plan output.
fn join_stage(output: &PlanOutput) -> &StageStats {
    output.stages.iter().find(|stage| stage.stage == "join").expect("the plan had a join stage")
}

/// The scan that feeds the join answers the same as the one that does not.
///
/// The lazy path probes the join **inside** the scan, before it copies
/// anything, so the rows the join does not match are never copied into the
/// dataset. The answer must not change: the two paths are held to the same
/// rows and the same sum, and the plan output says which path ran.
#[test]
fn the_scan_that_feeds_the_join_answers_the_same() {
    // The probe side's keys are 1..=5 and the build side holds 2, 3, 3, 5
    // and 9: one key twice, one key that no probe row has, and one probe
    // row (key 4) that has no partner.
    let probe_keys = vec![Some(1), Some(2), Some(3), Some(4), Some(5)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0), Some(50.0)];
    let build_keys = vec![Some(2), Some(3), Some(3), Some(5), Some(9)];
    let build_dates = vec![
        Some("1994-01-01"),
        Some("1994-02-01"),
        Some("1994-03-01"),
        Some("1994-04-01"),
        Some("1994-05-01"),
    ];

    let build_path = temp_path("streamed-build");
    write_build(&build_path, &build_keys, &build_dates);
    let probe_path = temp_path("streamed-probe");
    write_probe(&probe_path, &probe_keys, &probe_values);
    let plan = join_plan(&build_path, Vec::new());

    // The eager path materialises the probe side and probes it afterwards.
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let eager = engine_answer(&plan, &dataset);

    // The lazy path probes it inside the scan.
    let (lazy, output) = engine_answer_and_output(&plan, &probe_path);

    assert_eq!(lazy, eager, "the streamed scan answered differently from the materialising one");

    let rows = nested_loop(&probe_keys, &build_keys);
    let expected = reference_sum(&probe_values, &rows);
    assert_eq!(lazy.0, i64::try_from(rows.len()).expect("a row count fits in an i64"));
    assert_eq!(lazy.1, expected, "the join's rows differ from the nested loop's");

    let join = output.join.as_ref().expect("the plan had a join");
    assert!(join.probe_in_scan, "the scan did not feed the join");

    let _ = std::fs::remove_file(build_path);
    let _ = std::fs::remove_file(probe_path);
}

/// The two paths report the join's stage the same way.
///
/// The probe examined the same rows on both paths, and the join's stage says
/// so: the rows the probe **read** are the join's input, and the rows it
/// **kept** are its output.
///
/// Before this was fixed the cold path reported the rows that survived the
/// probe as its input -- the selection was over the dataset the probe had
/// already reduced -- and the two paths did not describe the same work. The
/// cold path's stage read `216783 -> 216783` while the resident path's read
/// `6000000 -> 216783`, for the same probe over the same six million rows.
#[test]
fn the_two_paths_report_the_join_s_the_same_work() {
    // The probe side's keys are 1..=5 and the build side holds 2, 3, 3, 5
    // and 9: one key twice, one key that no probe row has, and one probe
    // row (key 4) that has no partner.
    let probe_keys = vec![Some(1), Some(2), Some(3), Some(4), Some(5)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0), Some(50.0)];
    let build_keys = vec![Some(2), Some(3), Some(3), Some(5), Some(9)];
    let build_dates = vec![
        Some("1994-01-01"),
        Some("1994-02-01"),
        Some("1994-03-01"),
        Some("1994-04-01"),
        Some("1994-05-01"),
    ];

    let build_path = temp_path("stage-build");
    write_build(&build_path, &build_keys, &build_dates);
    let probe_path = temp_path("stage-probe");
    write_probe(&probe_path, &probe_keys, &probe_values);
    let plan = join_plan(&build_path, Vec::new());

    // The eager path materialises the probe side and probes it afterwards.
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let eager = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    // The lazy path probes it inside the scan.
    let (_, lazy) = engine_answer_and_output(&plan, &probe_path);

    let eager_stage = join_stage(&eager.plan_output);
    let lazy_stage = join_stage(&lazy);

    assert_eq!(
        eager_stage.rows_in, lazy_stage.rows_in,
        "the two paths disagree about the join's input"
    );
    assert_eq!(
        eager_stage.rows_out, lazy_stage.rows_out,
        "the two paths disagree about the join's output"
    );

    // The probe read all five probe rows, and kept the four that matched:
    // key 2 matches once, key 3 matches twice, and key 5 matches once.
    assert_eq!(eager_stage.rows_in, 5);
    assert_eq!(eager_stage.rows_out, 4);

    let _ = std::fs::remove_file(build_path);
    let _ = std::fs::remove_file(probe_path);
}

/// The join agrees with the nested loop, row for row and in order.
#[test]
fn the_hash_join_agrees_with_the_nested_loop() {
    // The probe side's keys are 1..=5 and the build side holds 2, 3, 3, 5
    // and 9: one key twice, one key that no probe row has, and one probe
    // row (key 4) that has no partner.
    let probe_keys = vec![Some(1), Some(2), Some(3), Some(4), Some(5)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0), Some(50.0)];
    let build_keys = vec![Some(2), Some(3), Some(3), Some(5), Some(9)];
    let build_dates = vec![
        Some("1994-01-01"),
        Some("1994-02-01"),
        Some("1994-03-01"),
        Some("1994-04-01"),
        Some("1994-05-01"),
    ];

    let path = temp_path("agree");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);
    let rows = nested_loop(&probe_keys, &build_keys);
    let expected = reference_sum(&probe_values, &rows);

    assert_eq!(count, i64::try_from(rows.len()).expect("a row count fits in an i64"));
    assert_eq!(sum, expected, "the join's rows differ from the nested loop's");

    let _ = std::fs::remove_file(path);
}

/// A probe key **below** the build table's base matches nothing.
///
/// The table is addressed by the key itself, and its base is the smallest key
/// the file's statistics report. A key below the base has no slot in the
/// table, and a table that wrapped its index round would answer with the
/// base key's count instead of with nothing.
#[test]
fn a_probe_key_below_the_table_s_base_matches_nothing() {
    // The build side's keys are 100 and 200, so the table's base is 100.
    let probe_keys = vec![Some(50), Some(100), Some(150), Some(200)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0)];
    let build_keys = vec![Some(100), Some(200)];
    let build_dates = vec![Some("1994-01-01"), Some("1994-02-01")];

    let path = temp_path("below-base");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);
    let rows = nested_loop(&probe_keys, &build_keys);
    let expected = reference_sum(&probe_values, &rows);

    assert_eq!(count, i64::try_from(rows.len()).expect("a row count fits in an i64"));
    assert_eq!(sum, expected, "the join's rows differ from the nested loop's");
    assert_eq!(count, 2, "only the two keys the build side holds match");

    let _ = std::fs::remove_file(path);
}

/// A build key more than 255 build rows carry still counts every one of them.
///
/// The table holds one byte a key, so a key more than 255 build rows carry
/// does not fit in its byte. The count does not fit, so the table used to be
/// abandoned -- and the answer was still right, but the probe paid for a
/// hash table. Now the count goes to an overflow map and the table keeps its
/// shape.
#[test]
fn a_build_key_more_than_255_rows_carry_still_counts_every_one() {
    let duplicates = 300_i32;
    let build_keys: Vec<Option<i64>> = (0..duplicates).map(|_| Some(7)).collect();
    let build_dates: Vec<Option<&str>> = (0..duplicates).map(|_| Some("1994-01-01")).collect();
    let probe_keys = vec![Some(7), Some(8)];
    let probe_values = vec![Some(10.0), Some(20.0)];

    let path = temp_path("overflow");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);
    let rows = nested_loop(&probe_keys, &build_keys);
    let expected = reference_sum(&probe_values, &rows);

    assert_eq!(count, i64::try_from(rows.len()).expect("a row count fits in an i64"));
    assert_eq!(sum, expected, "the join's rows differ from the nested loop's");
    assert_eq!(count, i64::from(duplicates));

    let _ = std::fs::remove_file(path);
}

/// The build side's own filter is applied **before** the table is hashed.
#[test]
fn the_build_filter_removes_rows_before_the_table_is_built() {
    let probe_keys = vec![Some(1), Some(2), Some(3), Some(4), Some(5)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0), Some(50.0)];
    // Two build rows are in 1994 and two are in 1995.
    let build_keys = vec![Some(2), Some(3), Some(3), Some(5)];
    let build_dates =
        vec![Some("1994-01-01"), Some("1994-02-01"), Some("1995-01-01"), Some("1995-02-01")];

    let path = temp_path("build-filter");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(
        &path,
        vec![Predicate::new(
            "o_orderdate",
            ComparisonOp::GtEq,
            ScalarValue::Utf8("1995-01-01".to_owned()),
        )],
    );

    let (count, sum) = engine_answer(&plan, &dataset);

    // Only the build rows of 1995 are in the table: key 3 (twice) and key 5.
    let surviving = vec![Some(3), Some(5)];
    let rows = nested_loop(&probe_keys, &surviving);
    let expected = reference_sum(&probe_values, &rows);

    assert_eq!(count, i64::try_from(rows.len()).expect("a row count fits in an i64"));
    assert_eq!(sum, expected, "the build filter did not remove the right rows");

    let _ = std::fs::remove_file(path);
}

/// A NULL key never matches, on either side.
#[test]
fn a_null_key_never_matches_on_either_side() {
    let probe_keys = vec![Some(1), None, Some(3), None];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0)];
    // The build side holds a NULL key too, and it must not match anything.
    let build_keys = vec![None, Some(1), Some(3), None];
    let build_dates =
        vec![Some("1994-01-01"), Some("1994-02-01"), Some("1994-03-01"), Some("1994-04-01")];

    let path = temp_path("null-key");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);
    let rows = nested_loop(&probe_keys, &build_keys);
    let expected = reference_sum(&probe_values, &rows);

    assert_eq!(count, i64::try_from(rows.len()).expect("a row count fits in an i64"));
    assert_eq!(sum, expected, "a NULL key matched something");

    let _ = std::fs::remove_file(path);
}

/// An empty build side matches nothing.
#[test]
fn an_empty_build_side_matches_nothing() {
    let probe_keys = vec![Some(1), Some(2)];
    let probe_values = vec![Some(10.0), Some(20.0)];

    let path = temp_path("empty-build");
    write_build(&path, &[], &[]);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);

    assert_eq!(count, 0, "an empty build side produced rows");
    assert_eq!(sum, None, "an empty build side produced a sum");

    let _ = std::fs::remove_file(path);
}

/// An empty probe side produces nothing.
#[test]
fn an_empty_probe_side_produces_nothing() {
    let build_keys = vec![Some(1), Some(2)];
    let build_dates = vec![Some("1994-01-01"), Some("1994-02-01")];

    let path = temp_path("empty-probe");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&[], &[]);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);

    assert_eq!(count, 0, "an empty probe side produced rows");
    assert_eq!(sum, None, "an empty probe side produced a sum");

    let _ = std::fs::remove_file(path);
}

/// A key that is in the build side twice matches **every** one of them.
///
/// This is the difference between an inner join and a semi-join, and it is
/// the case a hash table that forgets a duplicate gets wrong.
#[test]
fn a_duplicate_build_key_matches_every_one_of_its_rows() {
    let probe_keys = vec![Some(7)];
    let probe_values = vec![Some(70.0)];
    let build_keys = vec![Some(7), Some(7), Some(7)];
    let build_dates = vec![Some("1994-01-01"), Some("1994-02-01"), Some("1994-03-01")];

    let path = temp_path("duplicate");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);

    assert_eq!(count, 3, "the probe row did not match all three build rows");
    assert_eq!(sum, Some(210.0), "the joined rows are not the probe row's value three times");

    let _ = std::fs::remove_file(path);
}

/// The join keeps the probe side's row order, and the order is visible.
///
/// The values are chosen so that the sum depends on the order they are added
/// in: `1e16 + 1.0` rounds to `1e16`, so the sum of these three rows is zero
/// in this order and one in any other. A join that reordered the rows would
/// answer differently, bit for bit.
#[test]
fn the_join_keeps_the_probe_side_s_row_order() {
    let probe_keys = vec![Some(1), Some(2), Some(3)];
    let probe_values = vec![Some(1e16), Some(1.0), Some(-1e16)];
    let build_keys = vec![Some(3), Some(1), Some(2)];
    let build_dates = vec![Some("1994-01-01"), Some("1994-02-01"), Some("1994-03-01")];

    let path = temp_path("order");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&probe_keys, &probe_values);
    let plan = join_plan(&path, Vec::new());

    let (count, sum) = engine_answer(&plan, &dataset);

    assert_eq!(count, 3, "the join lost a row");
    let sum = sum.expect("the sum is not NULL");
    assert_eq!(sum.to_bits(), 0.0f64.to_bits(), "the rows were summed in another order: {sum}");

    let _ = std::fs::remove_file(path);
}

/// The lazy path and the eager path agree.
///
/// The lazy path reads the probe side from a file and pushes the plan's
/// predicates into the reader; the eager path reads it from memory. The
/// join must not be able to tell the difference.
#[test]
fn the_lazy_path_and_the_eager_path_agree() {
    let probe_keys = vec![Some(1), Some(2), None, Some(4)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0)];
    let build_keys = vec![Some(2), Some(2), Some(4), None];
    let build_dates =
        vec![Some("1994-01-01"), Some("1994-02-01"), Some("1994-03-01"), Some("1994-04-01")];

    let build_path = temp_path("both-build");
    write_build(&build_path, &build_keys, &build_dates);
    let probe_path = temp_path("both-probe");
    write_probe(&probe_path, &probe_keys, &probe_values);

    let plan = join_plan(&build_path, Vec::new());
    let eager = engine_answer(&plan, &probe_dataset(&probe_keys, &probe_values));
    let lazy = engine_answer_on(&plan, &probe_path);

    assert_eq!(eager, lazy, "the two paths answered differently");

    let _ = std::fs::remove_file(build_path);
    let _ = std::fs::remove_file(probe_path);
}

/// The plan output names the join's keys and its build side.
#[test]
fn the_plan_output_names_the_join() {
    let build_keys = vec![Some(2)];
    let build_dates = vec![Some("1994-01-01")];
    let path = temp_path("plan-output");
    write_build(&path, &build_keys, &build_dates);
    let dataset = probe_dataset(&[Some(2)], &[Some(20.0)]);
    let plan = join_plan(&path, Vec::new());

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let report = result.plan_output.join.as_ref().expect("the plan output reports the join");

    assert_eq!(report.probe_key, "l_orderkey");
    assert_eq!(report.build_key, "o_orderkey");
    assert_eq!(report.build_file, "columnar-engine-join-plan-output.parquet");
    assert_eq!(report.build_rows, 1, "the build side holds one row");
    assert_eq!(report.output_rows, 1, "the join produced one row");

    // The rendered plan names the join's keys and its build side, and the
    // optimisation order names the two stages the pipeline gained.
    let rendered = result.plan_output.to_string();
    assert!(
        rendered.contains("join: equi-join on l_orderkey = o_orderkey"),
        "the plan output is {rendered}"
    );
    assert!(
        rendered.contains("columnar-engine-join-plan-output.parquet"),
        "the plan output is {rendered}"
    );
    assert!(
        rendered.contains("late materialization -> join -> sort -> aggregate"),
        "the plan output is {rendered}"
    );

    let _ = std::fs::remove_file(path);
}

/// The published join query runs, and answers what the SQL says.
///
/// The query is the engine's own: `join-orders`, the hash join of
/// `lineitem` to `orders` on the order key, with the build side's own
/// filter -- the 1994 order dates -- applied before the table is built.
#[test]
fn the_published_join_query_runs() {
    // The probe side: three line items, with one that has no order.
    let probe_keys = vec![Some(1), Some(2), Some(3)];
    let probe_values = vec![Some(10.0), Some(20.0), Some(30.0)];
    // The build side: two orders in 1994 and one in 1995, plus one that no
    // line item points at.
    let build_keys = vec![Some(1), Some(2), Some(3), Some(9)];
    let build_dates =
        vec![Some("1994-01-01"), Some("1994-06-01"), Some("1995-01-01"), Some("1994-12-31")];

    let build_path = temp_path("published-build");
    write_build(&build_path, &build_keys, &build_dates);
    let probe_path = temp_path("published-probe");
    write_probe(&probe_path, &probe_keys, &probe_values);

    let definition = columnar_engine::queries::QUERIES
        .iter()
        .find(|definition| definition.query == columnar_engine::BuiltinQuery::JoinOrders)
        .expect("the join query is a builtin query");
    let plan = (definition.plan)(&build_path);

    let source = ParquetSource::open(&probe_path).expect("the source opens");
    let result = execute_on(&plan, &source, &scalar()).expect("the plan runs");
    let sum = match result.groups.first().map(|group| group.values.first()) {
        Some(Some(AggregateValue::Float(sum))) => *sum,
        other => panic!("unexpected aggregate {other:?}"),
    };

    // The orders of 1994 are keys 1, 2 and 9; key 3's order is in 1995 and
    // is not in the table, and key 9 has no line item. The joined rows are
    // the line items of keys 1 and 2.
    assert_eq!(sum.to_bits(), 30.0f64.to_bits(), "the join summed the wrong rows: {sum}");
    let join = result.plan_output.join.expect("the plan output reports the join");
    assert_eq!(join.output_rows, 2, "the join produced the wrong number of rows");

    let _ = std::fs::remove_file(build_path);
    let _ = std::fs::remove_file(probe_path);
}

/// A join whose two keys have different types is a plan error.
///
/// The engine never casts silently: joining an `int64` key to a `utf8` one
/// is an error rather than something to paper over.
#[test]
fn a_join_whose_keys_have_different_types_is_a_plan_error() {
    let path = temp_path("key-types");
    let schema = Arc::new(Schema::new(vec![
        Field::new("o_orderkey", DataType::Utf8, true),
        Field::new("o_orderdate", DataType::Utf8, true),
    ]));
    let keys: ArrayRef = Arc::new(StringArray::from(vec!["1", "2"]));
    let dates: ArrayRef = Arc::new(StringArray::from(vec!["1994-01-01", "1994-02-01"]));
    let batch = RecordBatch::try_new(schema.clone(), vec![keys, dates])
        .expect("the batch matches the schema");
    let file = File::create(&path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer starts");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is closed");

    let dataset = probe_dataset(&[Some(1)], &[Some(10.0)]);
    let plan = join_plan(&path, Vec::new());
    let error = execute(&plan, &dataset, &scalar()).expect_err("the join is rejected");

    assert!(
        matches!(error, columnar_engine::EngineError::TypeMismatch(_)),
        "the error was {error:?}"
    );

    // The lazy path rejects it too, and it rejects it **before** it reads
    // anything: the build side is opened first, and the two keys' types are
    // compared there.
    let probe_path = temp_path("key-types-probe");
    write_probe(&probe_path, &[Some(1)], &[Some(10.0)]);
    let source = ParquetSource::open(&probe_path).expect("the source opens");
    let error = execute_on(&plan, &source, &scalar()).expect_err("the join is rejected");
    assert!(
        matches!(error, columnar_engine::EngineError::TypeMismatch(_)),
        "the error was {error:?}"
    );

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(probe_path);
}

/// A join whose build key the build file does not have is a plan error.
///
/// The build side is a file of its own, and the key is named in it. A key the
/// file does not have is **not** a key of some default type: typing it `int64`
/// would let a plan join against a column that does not exist, and -- when the
/// probe's key is a `utf8` one -- would report a type mismatch for a column
/// that is not there at all.
#[test]
fn a_join_whose_build_key_is_absent_is_a_plan_error() {
    // The build file holds the key under another name, so the plan's build
    // key names a column the file does not have.
    let path = temp_path("absent-build-key");
    let schema = Arc::new(Schema::new(vec![
        Field::new("o_otherkey", DataType::Int64, true),
        Field::new("o_orderdate", DataType::Utf8, true),
    ]));
    let keys: ArrayRef = Arc::new(Int64Array::from(vec![1_i64, 2]));
    let dates: ArrayRef = Arc::new(StringArray::from(vec!["1994-01-01", "1994-02-01"]));
    let batch = RecordBatch::try_new(schema.clone(), vec![keys, dates])
        .expect("the batch matches the schema");
    let file = File::create(&path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer starts");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is closed");

    let dataset = probe_dataset(&[Some(1)], &[Some(10.0)]);
    let plan = join_plan(&path, Vec::new());
    let error = execute(&plan, &dataset, &scalar()).expect_err("the join is rejected");

    assert!(
        matches!(error, columnar_engine::EngineError::UnknownColumn(_)),
        "the error was {error:?}"
    );

    let _ = std::fs::remove_file(path);
}
