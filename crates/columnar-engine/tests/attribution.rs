//! The plan output's account of the query.
//!
//! The plan output is the engine's own account of its work, so it has to be
//! one: the stages it reports add up to the query's time, and the part of
//! the query that fell outside them is named rather than hidden.
//!
//! The test that would have caught the time the plan output did not name
//! lives here. It is a **ratio**, and the ratio is taken over the smallest
//! of several runs, because the time outside the stages can only grow: a run
//! the machine interrupted has a gap in it, and the smallest run is the one
//! that was not interrupted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use columnar_core::Encoding;
use columnar_engine::{
    execute, execute_on, Aggregate, AggregateSpec, AggregateValue, Dataset, ExecOptions,
    MemoryBudget, ParquetSource, QueryPlan, SortSpec,
};
use columnar_kernels::Flavor;

/// How many rows the test's dataset holds.
///
/// The account is a ratio, so the query has to be big enough for the ratio
/// to mean something: the time outside the stages is a few microseconds, and
/// a query that takes a millisecond says so.
const ROWS: usize = 200_000;

/// How many times the query is run.
const RUNS: usize = 5;

/// The most of the query the stages may fail to account for.
///
/// A hundredth of the query, or fifty microseconds -- whichever is more.
/// The floor is the engine's own overhead: the plan's resolution, the memory
/// estimate, and the handful of clock calls between the stages.
const TOLERANCE: Duration = Duration::from_micros(50);

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
    columnar_core::ColumnChunk::new(
        name,
        encoding,
        columnar_core::NullBitmap::with_nulls(len, nulls),
    )
}

/// A row's number, as the double the column holds.
///
/// The row numbers are well inside a `u32`, so the narrowing is exact.
fn as_double(row: usize) -> f64 {
    f64::from(u32::try_from(row).unwrap_or(u32::MAX))
}

/// The dataset: a sort key and a value, one row per number.
fn dataset() -> Dataset {
    let keys: Vec<f64> = (0..ROWS).map(|row| as_double(row % 1000)).collect();
    let values: Vec<f64> = (0..ROWS).map(as_double).collect();
    Dataset::from_chunks(vec![
        chunk("k", Encoding::Float64 { values: keys }, &[]),
        chunk("v", Encoding::Float64 { values }, &[]),
    ])
}

/// The plan: a sort with a limit, and the aggregate over the rows it keeps.
fn plan() -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![
                Aggregate::CountStar,
                Aggregate::Sum { expr: columnar_core::Expr::column("v") },
            ],
        }),
        join: None,
        sort: Some(SortSpec { column: "k".to_owned(), descending: true, limit: Some(10) }),
    }
}

/// A path for a test's temporary file.
fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("columnar-engine-attribution-{name}.parquet"))
}

/// The smallest gap between the query and its stages, over several runs.
///
/// The query is run [`RUNS`] times and the **smallest** gap is the figure:
/// the gap can only grow -- a run the machine interrupted has a gap in it --
/// so the smallest run is the one that was not interrupted.
fn smallest_gap(plan: &QueryPlan, dataset: &Dataset) -> (Duration, Duration) {
    let mut smallest: Option<(Duration, Duration)> = None;
    for _ in 0..RUNS {
        let result = execute(plan, dataset, &scalar()).expect("the plan runs");
        let attribution = result.plan_output.attribution;
        let pair = (attribution.outside, attribution.elapsed);
        smallest = Some(match smallest {
            Some(best) if best.0 <= pair.0 => best,
            _ => pair,
        });
    }
    smallest.expect("the query ran at least once")
}

/// The smallest gap between the query and its stages, over the **cold** path.
///
/// The cold path is the one the published campaigns measure: the file is read
/// as part of the query, the plan is resolved against the file, and the join
/// streams its probe inside the scan. It is a different code path, so the
/// account has to hold there as well.
fn smallest_gap_cold(plan: &QueryPlan, source: &ParquetSource) -> (Duration, Duration) {
    let mut smallest: Option<(Duration, Duration)> = None;
    for _ in 0..RUNS {
        let result = execute_on(plan, source, &scalar()).expect("the plan runs");
        let attribution = result.plan_output.attribution;
        let pair = (attribution.outside, attribution.elapsed);
        smallest = Some(match smallest {
            Some(best) if best.0 <= pair.0 => best,
            _ => pair,
        });
    }
    smallest.expect("the query ran at least once")
}

/// The plan output accounts for the query.
///
/// The stages add up to the query's time, to within the engine's own
/// overhead. A stage table that leaves a hole in the query is an account
/// with a hole in it, and the hole is where the engine's work goes to hide.
#[test]
fn the_plan_output_accounts_for_the_query() {
    let dataset = dataset();
    let plan = plan();
    let (outside, elapsed) = smallest_gap(&plan, &dataset);

    let tolerance = std::cmp::max(elapsed / 100, TOLERANCE);
    assert!(
        outside <= tolerance,
        "the plan output left {:.3} ms of a {:.3} ms query unnamed, and the tolerance is {:.3} ms",
        outside.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0,
        tolerance.as_secs_f64() * 1000.0,
    );
}

/// The plan output accounts for the query on the **cold** path too.
///
/// The cold path is the one the published campaigns measure: the file is read
/// as part of the query, and the join streams its probe inside the scan. It
/// is a different code path from the resident one, so the account has to hold
/// there as well -- and it is the path where the old code's sort left
/// **4.3 ms** outside its stages.
#[test]
fn the_plan_output_accounts_for_the_query_on_the_cold_path() {
    let path = temp_path("probe");
    write_probe(&path);

    let source = ParquetSource::open(&path).expect("the source opens");
    let plan = plan();
    let (outside, elapsed) = smallest_gap_cold(&plan, &source);

    let tolerance = std::cmp::max(elapsed / 100, TOLERANCE);
    assert!(
        outside <= tolerance,
        "the plan output left {:.3} ms of a {:.3} ms query unnamed, and the tolerance is {:.3} ms",
        outside.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0,
        tolerance.as_secs_f64() * 1000.0,
    );
}

/// The cold path gives the same answer as the resident one.
///
/// The account is only worth anything if the two paths still agree: the same
/// rows, the same sort, the same ten rows kept.
#[test]
fn the_cold_path_gives_the_same_answer_as_the_resident_one() {
    let path = temp_path("probe");
    write_probe(&path);

    let source = ParquetSource::open(&path).expect("the source opens");
    let plan = plan();
    let cold = execute_on(&plan, &source, &scalar()).expect("the cold path answers");
    let dataset = Dataset::read_parquet(&path).expect("the dataset reads");
    let resident = execute(&plan, &dataset, &scalar()).expect("the resident path answers");

    assert_eq!(cold.groups, resident.groups, "the two paths disagreed on the answer");
}

/// The account is reported, not just kept.
///
/// The plan output says what it accounted for and what it did not, so a
/// reader never has to work the difference out from the stage table.
#[test]
fn the_plan_output_says_what_it_accounted_for() {
    let dataset = dataset();
    let plan = plan();
    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let attribution = result.plan_output.attribution;

    let accounted: Duration = result.stages.iter().map(|stage| stage.elapsed).sum();
    assert_eq!(attribution.accounted, accounted, "the account is the stage table's total");
    assert_eq!(
        attribution.outside,
        attribution.elapsed.saturating_sub(accounted),
        "the outside is the query less the stages"
    );

    let printed = result.plan_output.to_string();
    assert!(
        printed.contains("accounted:"),
        "the plan output does not say what it accounted for:\n{printed}"
    );
}

/// The sort is handed the same rows, and the answer does not change.
///
/// The sort keeps the ten largest keys, and the aggregate sums the values
/// of the rows those keys name. The reference is the rule written out, and
/// the engine has to agree with it value for value.
#[test]
fn the_sort_is_handed_the_same_rows_and_the_answer_does_not_change() {
    let dataset = dataset();
    let plan = plan();
    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    assert_eq!(count, 10, "the sort did not keep ten rows");

    // The largest key is 999, and two hundred rows have it. The sort keeps
    // ten of them, and the tie is broken by the row's own number, so the
    // rows are the first ten: 999, 1999, and so on.
    let expected: f64 = (0..10).map(|index| as_double(999 + 1000 * index)).sum();
    let sum = match group.values.get(1) {
        Some(AggregateValue::Float(sum)) => *sum,
        Some(AggregateValue::Null) => 0.0,
        other => panic!("unexpected sum {other:?}"),
    };
    assert!(
        (sum - expected).abs() < 1e-6,
        "the sort kept the wrong rows: {sum} against {expected}"
    );
}

/// The join is handed the same rows, and the account holds there too.
///
/// The join's build side is a file of its own, and opening it is the
/// join's own work: it is reported in the join's stage, so the account
/// holds for the join as well as for the sort.
#[test]
fn the_join_is_handed_the_same_rows_and_the_account_holds() {
    let build = temp_path("build");
    write_build(&build, 16);

    let dataset = dataset();
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
        join: Some(columnar_engine::JoinSpec {
            build_path: build.clone(),
            probe_key: "k".to_owned(),
            build_key: "o_orderkey".to_owned(),
            build_filter: Vec::new(),
        }),
        sort: None,
    };

    let (outside, elapsed) = smallest_gap(&plan, &dataset);
    let tolerance = std::cmp::max(elapsed / 100, TOLERANCE);
    assert!(
        outside <= tolerance,
        "the plan output left {:.3} ms of a {:.3} ms query unnamed, and the tolerance is {:.3} ms",
        outside.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0,
        tolerance.as_secs_f64() * 1000.0,
    );

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    // Every row's key is below a thousand, and the build side holds the
    // keys 0..16, so each of the sixteen build keys matches two hundred
    // rows: three thousand two hundred rows in total.
    assert_eq!(count, 3_200, "the join matched the wrong rows");
}

/// Writes the build side's file: sixteen order keys.
///
/// The keys are doubles, because the probe side's key is a double: the
/// engine never casts a key silently.
fn write_build(path: &Path, keys: i64) {
    use std::sync::Arc;

    use arrow::array::{ArrayRef, Float64Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    let schema = Arc::new(Schema::new(vec![Field::new("o_orderkey", DataType::Float64, true)]));
    let values: ArrayRef = Arc::new(Float64Array::from(
        (0..keys)
            .map(|key| f64::from(u32::try_from(key).unwrap_or(u32::MAX)))
            .collect::<Vec<f64>>(),
    ));
    let batch = arrow::record_batch::RecordBatch::try_new(schema.clone(), vec![values])
        .expect("the batch matches the schema");
    let file = std::fs::File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer starts");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is closed");
}

/// Writes the probe side's file: the sort key and the value, one row per
/// number.
///
/// The file is the same data the in-memory dataset holds, so the two paths
/// can be compared row for row.
fn write_probe(path: &Path) {
    use std::sync::Arc;

    use arrow::array::{ArrayRef, Float64Array};
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    let schema = Arc::new(Schema::new(vec![
        Field::new("k", DataType::Float64, true),
        Field::new("v", DataType::Float64, true),
    ]));
    let keys: ArrayRef = Arc::new(Float64Array::from(
        (0..ROWS).map(|row| as_double(row % 1000)).collect::<Vec<f64>>(),
    ));
    let values: ArrayRef =
        Arc::new(Float64Array::from((0..ROWS).map(as_double).collect::<Vec<f64>>()));
    let batch = arrow::record_batch::RecordBatch::try_new(schema.clone(), vec![keys, values])
        .expect("the batch matches the schema");
    let file = std::fs::File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer starts");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is closed");
}
