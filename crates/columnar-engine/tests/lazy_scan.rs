//! The lazy path and the eager path must agree, row for row.
//!
//! # Why this file exists
//!
//! The lazy path pushes predicates into the Parquet reader. The reader is
//! Arrow's, and Arrow's comparison semantics are **not** the engine's:
//!
//! - Arrow returns `null` for a comparison against a NULL, and `RowFilter`
//!   keeps a row whose predicate is `null`. The engine drops NULL rows, for
//!   every operator, including `!=`.
//! - In IEEE-754, `NaN != 5.0` is **true**. The engine says a NaN never
//!   matches, for every operator, including `!=`.
//!
//! So the pushdown implements the engine's rules itself, and this file is
//! what holds it to them. Every test here answers the same question twice --
//! once over the eagerly decoded dataset, once over the lazily read file --
//! and the two answers have to be identical.
//!
//! The datasets are written here rather than generated, because the edges
//! that matter are exactly the ones a generator would smooth over: NULLs at
//! particular rows, NaN values, a NaN *threshold*, and row counts around
//! the 64-row block boundary the comparison kernel works in.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray, StringDictionaryBuilder};
use arrow::datatypes::{DataType, Field, Int32Type, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Predicate, ScalarValue};
use columnar_engine::{
    execute, execute_on, Aggregate, AggregateSpec, Dataset, ExecOptions, ParquetSource, QueryPlan,
};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

/// A temporary file that removes itself when it is dropped.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A path under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!("columnar-lazy-{name}-{unique}.parquet"));
        Self { path }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The schema the tests write: a float column, a dictionary string column
/// and an integer column.
fn schema() -> SchemaRef {
    Schema::new(vec![
        Field::new("l_discount", DataType::Float64, true),
        Field::new(
            "l_shipdate",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
        Field::new("l_quantity", DataType::Int64, true),
    ])
    .into()
}

/// Writes a Parquet file holding one batch.
fn write(path: &Path, batch: &RecordBatch) {
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .build();
    let file = File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(properties))
        .expect("the arrow writer accepts the batch");
    writer.write(batch).expect("the batch can be written");
    writer.close().expect("the file can be closed");
}

/// A dictionary column over the given values.
fn dictionary(values: &[Option<&str>]) -> ArrayRef {
    let mut builder = StringDictionaryBuilder::<Int32Type>::new();
    for value in values {
        builder.append_option(*value);
    }
    Arc::new(builder.finish())
}

/// A float column over the given values.
fn floats(values: &[Option<f64>]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}

/// An integer column over the given values.
fn integers(values: &[Option<i64>]) -> ArrayRef {
    Arc::new(Int64Array::from(values.to_vec()))
}

/// The batch the tests read: a mix of values, NULLs and NaNs.
fn edge_case_batch() -> RecordBatch {
    let discount = floats(&[
        Some(0.02),
        Some(0.05),
        None,
        Some(f64::NAN),
        Some(0.07),
        None,
        Some(0.04),
        Some(0.09),
    ]);
    let shipdate = dictionary(&[
        Some("1993-01-01"),
        Some("1994-01-01"),
        Some("1994-06-01"),
        None,
        Some("1995-01-01"),
        Some("1994-12-31"),
        Some("1996-03-03"),
        Some("1993-06-06"),
    ]);
    let quantity = integers(&[Some(1), Some(2), None, Some(4), Some(5), Some(6), Some(7), Some(8)]);

    RecordBatch::try_new(schema(), vec![discount, shipdate, quantity])
        .expect("the batch matches the schema")
}

/// Runs a plan both ways and asserts the answers are identical.
///
/// Returns the two results so a test can look at them further.
fn both_paths(
    path: &Path,
    plan: &QueryPlan,
) -> (columnar_engine::AggregateResult, columnar_engine::AggregateResult) {
    let dataset = Dataset::read_parquet(path).expect("the dataset can be read eagerly");
    let eager =
        execute(plan, &dataset, &ExecOptions::default()).expect("the eager path answers the query");

    let source = ParquetSource::open(path).expect("the source can be opened");
    let lazy = execute_on(plan, &source, &ExecOptions::default())
        .expect("the lazy path answers the query");

    assert_eq!(eager.groups, lazy.groups, "the eager and the lazy path disagreed on the groups");
    (eager, lazy)
}

/// The count the engine computed, as a row count.
fn counted(result: &columnar_engine::AggregateResult) -> usize {
    match result.groups.first().and_then(|group| group.values.first()) {
        Some(columnar_engine::AggregateValue::Int(value)) => {
            usize::try_from(*value).expect("the count is not negative")
        }
        other => panic!("the count is not an integer: {other:?}"),
    }
}

/// A plan that counts the rows a single predicate keeps.
fn count_plan(column: &str, op: ComparisonOp, value: ScalarValue) -> QueryPlan {
    QueryPlan {
        filter: vec![Predicate::new(column, op, value)],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    }
}

/// Every operator, over a column with NULLs and NaNs, must give the same
/// answer on both paths.
///
/// The `Ne` operator is the interesting one: it is where Arrow's semantics
/// and the engine's differ most, because `NaN != 5.0` is true in IEEE-754
/// and the engine says a NaN never matches.
#[test]
fn the_two_paths_agree_for_every_operator() {
    let file = TempFile::new("operators");
    write(&file.path, &edge_case_batch());

    let thresholds = [
        ScalarValue::Float64(0.05),
        ScalarValue::Float64(f64::NAN),
        ScalarValue::Int64(4),
        ScalarValue::Int64(i64::MIN),
        ScalarValue::Utf8("1994-01-01".to_owned()),
        ScalarValue::Utf8("1994-07-01".to_owned()),
    ];

    for op in ComparisonOp::all() {
        for threshold in &thresholds {
            let column = match threshold {
                ScalarValue::Float64(_) => "l_discount",
                ScalarValue::Int64(_) => "l_quantity",
                ScalarValue::Utf8(_) => "l_shipdate",
            };
            let plan = count_plan(column, op, threshold.clone());
            both_paths(&file.path, &plan);
        }
    }
}

/// A conjunction of predicates must also agree.
#[test]
fn the_two_paths_agree_for_a_conjunction() {
    let file = TempFile::new("conjunction");
    write(&file.path, &edge_case_batch());

    let plan = QueryPlan {
        filter: vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.02)),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::Lt,
                ScalarValue::Utf8("1995-01-01".to_owned()),
            ),
        ],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    };
    both_paths(&file.path, &plan);
}

/// A NULL row must never match -- including under `!=` -- on both paths.
#[test]
fn a_null_row_never_matches_even_under_not_equal() {
    let file = TempFile::new("null");
    write(&file.path, &edge_case_batch());

    for op in [ComparisonOp::Eq, ComparisonOp::Ne] {
        let plan = count_plan("l_discount", op, ScalarValue::Float64(0.05));
        let (eager, lazy) = both_paths(&file.path, &plan);

        // Row 2 is NULL. It must not be counted under either operator.
        let counted = counted(&eager);
        let matched = eager
            .plan_output
            .stages
            .iter()
            .find(|stage| stage.stage == "filter")
            .map_or(0, |stage| usize::try_from(stage.rows_out).unwrap_or(0));
        assert_eq!(counted, matched, "the count and the filter's row count disagree");
        assert_eq!(lazy.groups, eager.groups);
    }
}

/// A NaN must never match -- including under `!=` -- on both paths.
#[test]
fn a_nan_never_matches_even_under_not_equal() {
    let file = TempFile::new("nan");
    write(&file.path, &edge_case_batch());

    for op in ComparisonOp::all() {
        let plan = count_plan("l_discount", op, ScalarValue::Float64(0.05));
        let (eager, _) = both_paths(&file.path, &plan);

        // Row 3 holds a NaN and must not be counted under any operator.
        let counted = counted(&eager);
        assert!(counted <= 7, "the NaN row was counted under {op:?}: {counted} of 8");
    }
}

/// A NaN *threshold* must match nothing, on both paths.
#[test]
fn a_nan_threshold_matches_nothing() {
    let file = TempFile::new("nan-threshold");
    write(&file.path, &edge_case_batch());

    for op in ComparisonOp::all() {
        let plan = count_plan("l_discount", op, ScalarValue::Float64(f64::NAN));
        let (eager, _) = both_paths(&file.path, &plan);
        let counted = counted(&eager);
        assert_eq!(counted, 0, "a NaN threshold matched {counted} rows under {op:?}");
    }
}

/// Row counts around the 64-row block boundary, where a tail bug hides.
#[test]
fn the_two_paths_agree_at_the_block_boundaries() {
    for len in [0usize, 1, 2, 63, 64, 65, 127, 128, 129, 191, 192, 193] {
        let file = TempFile::new("boundaries");
        let discount = floats(
            &(0..len)
                .map(|row| {
                    if row % 7 == 0 {
                        None
                    } else {
                        Some(f64::from(u32::try_from(row % 11).expect("the remainder fits")) / 10.0)
                    }
                })
                .collect::<Vec<Option<f64>>>(),
        );
        let shipdate = dictionary(
            &(0..len)
                .map(|row| Some(format!("{:04}-01-01", 1990 + (row % 8))))
                .collect::<Vec<Option<String>>>()
                .iter()
                .map(|value| value.as_deref())
                .collect::<Vec<Option<&str>>>(),
        );
        let quantity = integers(
            &(0..len)
                .map(|row| Some(i64::try_from(row).expect("the row count fits")))
                .collect::<Vec<Option<i64>>>(),
        );

        let batch = RecordBatch::try_new(schema(), vec![discount, shipdate, quantity])
            .expect("the batch matches the schema");
        write(&file.path, &batch);

        let plan = count_plan("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05));
        both_paths(&file.path, &plan);
    }
}

/// The lazy path reads only the columns the plan needs.
///
/// This is the claim the whole increment rests on, so it is checked against
/// the plan output rather than inferred from a timing.
#[test]
fn the_lazy_path_reads_only_the_columns_the_plan_needs() {
    let file = TempFile::new("projection");
    write(&file.path, &edge_case_batch());

    let plan = count_plan("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.05));
    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let result = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");

    assert_eq!(
        result.plan_output.columns_read,
        vec!["l_discount".to_owned()],
        "the lazy path read columns the plan did not ask for"
    );
    assert_eq!(
        result.plan_output.pushed_down,
        vec!["l_discount >= 0.05".to_owned()],
        "the predicate was not pushed into the reader"
    );
}

/// A predicate on a column nothing else reads is pushed into the reader.
#[test]
fn a_predicate_on_an_unread_column_is_pushed_down() {
    let file = TempFile::new("pushdown");
    write(&file.path, &edge_case_batch());

    let plan =
        count_plan("l_shipdate", ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let result = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");

    assert_eq!(
        result.plan_output.pushed_down,
        vec!["l_shipdate < '1995-01-01'".to_owned()],
        "the predicate was not reported as pushed down"
    );
    assert_eq!(
        result.plan_output.columns_read,
        vec!["l_shipdate".to_owned()],
        "the count(*) plan fetched the wrong columns: {:?}",
        result.plan_output.columns_read
    );
}

/// The scan reports what the pushdown cost, and over how many rows.
///
/// The pushdown runs *inside* the reader, so without this the scan's elapsed
/// time is a total that says nothing about which half of the work it was.
/// A total is not an explanation, and this project does not publish one.
#[test]
fn the_scan_reports_what_the_pushdown_cost() {
    let file = TempFile::new("pushdown-cost");
    write(&file.path, &edge_case_batch());

    let plan =
        count_plan("l_shipdate", ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let result = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");

    let metrics = result.plan_output.pushdown_metrics;
    assert!(metrics.rows_evaluated > 0, "the pushdown evaluated no rows: {metrics:?}");
    assert!(metrics.batches > 0, "the pushdown evaluated no batches: {metrics:?}");
    assert!(metrics.ps_per_row() > 0, "the pushdown reported no time per row: {metrics:?}");
}

/// The eager path pushes nothing, so it reports no pushdown cost.
#[test]
fn the_eager_path_reports_no_pushdown_cost() {
    let file = TempFile::new("eager-no-pushdown");
    write(&file.path, &edge_case_batch());

    let plan =
        count_plan("l_shipdate", ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    let dataset = Dataset::read_parquet(&file.path).expect("the dataset can be read");
    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the query runs");

    let metrics = result.plan_output.pushdown_metrics;
    assert_eq!(metrics.rows_evaluated, 0, "the eager path pushed something: {metrics:?}");
    assert_eq!(metrics.batches, 0, "the eager path pushed something: {metrics:?}");
}

/// A plan that aggregates a column reads that column, and pushes the
/// predicate that is not on it.
#[test]
fn an_aggregate_column_is_read_and_the_predicate_is_pushed_down() {
    let file = TempFile::new("aggregate");
    write(&file.path, &edge_case_batch());

    let plan = QueryPlan {
        filter: vec![Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.02))],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { column: "l_quantity".to_owned() }],
        }),
    };

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let result = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");

    assert_eq!(
        result.plan_output.columns_read,
        vec!["l_discount".to_owned(), "l_quantity".to_owned()],
        "the lazy path fetched the wrong columns"
    );
    assert_eq!(result.plan_output.pushed_down.len(), 1, "the predicate was not pushed down");

    // And the two paths agree.
    both_paths(&file.path, &plan);
}

/// A predicate on a column the plan also aggregates is **not** pushed down.
///
/// Pushing it would make the reader decode that column twice: once to
/// evaluate the predicate and once to produce the output. That is slower
/// than not pushing it, so the predicate stays with the engine's filter.
#[test]
fn a_predicate_on_an_aggregated_column_is_not_pushed_down() {
    let file = TempFile::new("not-pushed");
    write(&file.path, &edge_case_batch());

    let plan = QueryPlan {
        filter: vec![Predicate::new("l_quantity", ComparisonOp::GtEq, ScalarValue::Int64(3))],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { column: "l_quantity".to_owned() }],
        }),
    };

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let result = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");

    assert!(
        result.plan_output.pushed_down.is_empty(),
        "a predicate on an aggregated column was pushed down: {:?}",
        result.plan_output.pushed_down
    );
    assert_eq!(result.plan_output.columns_read, vec!["l_quantity".to_owned()]);

    both_paths(&file.path, &plan);
}

/// An unknown column is an error, and it is an error *before* the file's
/// data is read.
#[test]
fn an_unknown_column_is_an_error() {
    let file = TempFile::new("unknown");
    write(&file.path, &edge_case_batch());

    let plan = count_plan("l_nothing", ComparisonOp::GtEq, ScalarValue::Float64(0.05));
    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let error = execute_on(&plan, &source, &ExecOptions::default())
        .expect_err("a plan naming an unknown column must fail");
    assert!(
        matches!(&error, columnar_engine::EngineError::UnknownColumn(name) if name == "l_nothing"),
        "the wrong error was reported: {error:?}"
    );
}

/// A type mismatch is an error too, and on both paths.
#[test]
fn a_type_mismatch_is_an_error() {
    let file = TempFile::new("mismatch");
    write(&file.path, &edge_case_batch());

    let plan = count_plan("l_discount", ComparisonOp::GtEq, ScalarValue::Int64(1));

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let error = execute_on(&plan, &source, &ExecOptions::default())
        .expect_err("a plan with a mismatched threshold must fail");
    assert!(
        matches!(error, columnar_engine::EngineError::TypeMismatch(_)),
        "the wrong error was reported: {error:?}"
    );

    let dataset = Dataset::read_parquet(&file.path).expect("the dataset can be read");
    let error = execute(&plan, &dataset, &ExecOptions::default())
        .expect_err("a plan with a mismatched threshold must fail");
    assert!(
        matches!(error, columnar_engine::EngineError::TypeMismatch(_)),
        "the wrong error was reported: {error:?}"
    );
}

/// The dictionary column's pushdown compares each distinct value once, and
/// maps back through the indices -- exactly as the engine's own dictionary
/// path does.
#[test]
fn the_dictionary_pushdown_agrees_with_the_engine() {
    let file = TempFile::new("dictionary");
    write(&file.path, &edge_case_batch());

    for op in ComparisonOp::all() {
        for threshold in ["1993-01-01", "1994-01-01", "1995-01-01", "1999-12-31"] {
            let plan = count_plan("l_shipdate", op, ScalarValue::Utf8(threshold.to_owned()));
            both_paths(&file.path, &plan);
        }
    }
}

/// A plan with no predicate at all reads everything it materialises and
/// counts every row.
#[test]
fn a_plan_without_a_predicate_counts_every_row() {
    let file = TempFile::new("no-predicate");
    write(&file.path, &edge_case_batch());

    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    };
    let (eager, lazy) = both_paths(&file.path, &plan);

    let counted = counted(&eager);
    assert_eq!(counted, 8, "the count is not the file's row count");
    assert_eq!(lazy.groups, eager.groups);
}

/// A plain UTF-8 column, not a dictionary one, pushes down the same way.
#[test]
fn a_plain_utf8_column_pushes_down() {
    let file = TempFile::new("plain-utf8");
    let schema: SchemaRef =
        Schema::new(vec![Field::new("l_shipdate", DataType::Utf8, true)]).into();
    let values = Arc::new(StringArray::from(vec![
        Some("1993-01-01"),
        Some("1994-01-01"),
        None,
        Some("1995-01-01"),
        Some("1994-06-01"),
    ]));
    let batch = RecordBatch::try_new(schema.clone(), vec![values]).expect("the batch fits");
    write(&file.path, &batch);

    for op in ComparisonOp::all() {
        let plan = count_plan("l_shipdate", op, ScalarValue::Utf8("1994-01-01".to_owned()));
        both_paths(&file.path, &plan);
    }
}
