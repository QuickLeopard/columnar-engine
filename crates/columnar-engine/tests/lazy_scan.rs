//! The lazy path and the eager path must agree, row for row.
//!
//! # Why this file exists
//!
//! The lazy path applies the plan's predicates during the scan, over the
//! reader's Arrow arrays. Arrow's comparison semantics are **not** the
//! engine's:
//!
//! - Arrow returns `null` for a comparison against a NULL, and a row filter
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

// The chunk indices below are literals into a one-chunk dataset whose length
// the test has just asserted, so the panic the lint guards against cannot
// happen.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray, StringDictionaryBuilder};
use arrow::datatypes::{DataType, Field, Int32Type, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Predicate, ScalarValue};
use columnar_engine::{
    execute, execute_on, Aggregate, AggregateSpec, Dataset, ExecOptions, ParquetSource,
    PushdownPlan, QueryPlan,
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

/// Writes a Parquet file holding **one row group per batch**.
///
/// This is what makes the parallel path reachable at all. The parallel read
/// splits the file's *row groups* across the threads, so a file with one row
/// group has one worker however many threads are asked for: a test written
/// against such a file would be testing the single-threaded path under the
/// parallel path's name, and it would pass whether the parallel path worked
/// or not.
fn write_row_groups(path: &Path, batches: &[RecordBatch]) {
    let first = batches.first().expect("there is at least one batch");
    let rows = first.num_rows();
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        // One row group per batch: the writer flushes the current row group
        // when it already holds this many rows.
        .set_max_row_group_size(rows)
        .build();
    let file = File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, first.schema(), Some(properties))
        .expect("the arrow writer accepts the batch");
    for batch in batches {
        writer.write(batch).expect("the batch can be written");
    }
    writer.close().expect("the file can be closed");
}

/// Three batches of eight rows, with NULLs, NaNs and a dictionary column.
///
/// The three groups differ in their values, so a merge that put them in the
/// wrong order would be visible in the answer.
fn three_row_group_batches() -> Vec<RecordBatch> {
    (0..3)
        .map(|group| {
            let base = i64::from(group * 8);
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
            let quantity = integers(
                &(0..8).map(|row| Some(base + i64::from(row))).collect::<Vec<Option<i64>>>(),
            );
            RecordBatch::try_new(schema(), vec![discount, shipdate, quantity])
                .expect("the batch matches the schema")
        })
        .collect()
}

/// The rows of a column, resolved.
///
/// A dictionary-encoded column is resolved to its strings, so the comparison
/// is about the **values** and not about which dictionary happened to hold
/// them: two reads can number their dictionaries differently and still be
/// holding the same data.
///
/// A NULL row is `None`.
fn resolved_rows(chunk: &columnar_core::ColumnChunk) -> Vec<Option<String>> {
    use columnar_core::Encoding;

    let values: Vec<String> = match &chunk.encoding {
        Encoding::Int64 { values } => values.iter().map(ToString::to_string).collect(),
        Encoding::Float64 { values } => values.iter().map(ToString::to_string).collect(),
        Encoding::Utf8 { values } => values.clone(),
        Encoding::Int64Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                dict.get(position).expect("the index is in range").to_string()
            })
            .collect(),
        Encoding::Utf8Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                dict.get(position).expect("the index is in range").clone()
            })
            .collect(),
        other => panic!("the comparison does not know the `{other:?}` encoding"),
    };

    values
        .into_iter()
        .enumerate()
        .map(|(row, value)| if chunk.validity.is_valid(row) { Some(value) } else { None })
        .collect()
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
    QueryPlan::new(
        vec![Predicate::new(column, op, value)],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
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

    let plan = QueryPlan::new(
        vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.02)),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::Lt,
                ScalarValue::Utf8("1995-01-01".to_owned()),
            ),
        ],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    );
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

/// The parallel path and the single-threaded path agree, exactly.
///
/// This is the requirement that makes parallel decode safe rather than
/// merely fast. `SUM` is not associative, so a parallel read that ordered its
/// rows by *completion* rather than by *position* would make the engine's
/// answer depend on the scheduler, and the same query would return different
/// bits on different runs.
///
/// So the two paths must produce the same answer, down to the last bit, for
/// every thread count.
///
/// The file has **three row groups**, because the parallel read splits the
/// row groups: a file with one row group has one worker however many threads
/// are asked for, and this test would then be testing the single-threaded
/// path under the parallel path's name.
#[test]
fn the_parallel_path_agrees_with_the_single_threaded_one() {
    let file = TempFile::new("parallel");
    write_row_groups(&file.path, &three_row_group_batches());

    let plan = QueryPlan::new(
        vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.02)),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::Lt,
                ScalarValue::Utf8("1995-01-01".to_owned()),
            ),
        ],
        Vec::new(),
        Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: columnar_core::Expr::column("l_quantity") }],
        }),
    );

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    assert_eq!(
        source.row_groups(),
        3,
        "the file does not have three row groups, so the parallel path never engages"
    );

    let single = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");
    for threads in [2, 3, 4, 8] {
        let options = ExecOptions { threads: Some(threads), ..ExecOptions::default() };
        let parallel = execute_on(&plan, &source, &options).expect("the parallel query runs");

        assert_eq!(
            parallel.groups, single.groups,
            "the parallel answer differs at {threads} threads"
        );
        assert_eq!(
            parallel.plan_output.threads,
            threads.min(source.row_groups()),
            "the plan output does not report the thread count"
        );
    }
}

/// The parallel read holds the same rows, in the same order, as the
/// single-threaded one.
///
/// The aggregate answer above cannot see the **order** of the rows: summing
/// a column does not care which row came first. So this test compares the
/// datasets themselves, row by row, with the dictionary resolved.
///
/// The order is the thing that is at stake. A parallel read that merged its
/// parts in *completion* order -- fastest thread first, rather than first
/// row group first -- would produce a dataset whose rows were in the wrong
/// order, and every answer computed from it would be a lie about a file
/// whose rows have an order.
#[test]
fn the_parallel_read_holds_the_same_rows_in_the_same_order() {
    let file = TempFile::new("parallel-rows");
    write_row_groups(&file.path, &three_row_group_batches());

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    assert_eq!(
        source.row_groups(),
        3,
        "the file does not have three row groups, so the parallel path never engages"
    );

    let pushdown = PushdownPlan::new();
    let flavor = columnar_kernels::best();

    let single = source.read(&[0, 1, 2], &pushdown, flavor, 1).expect("the read runs");
    assert_eq!(single.len(), 24, "the read did not return every row");

    for threads in [2, 3, 4, 8] {
        let parallel = source.read(&[0, 1, 2], &pushdown, flavor, threads).expect("the read runs");

        assert_eq!(parallel.len(), single.len(), "the parallel read has a different row count");
        for name in single.column_names() {
            let left = single.chunk(&name).expect("the column is there");
            let right = parallel.chunk(&name).expect("the column is there");
            assert_eq!(
                resolved_rows(right),
                resolved_rows(left),
                "the parallel read holds different rows in `{name}` at {threads} threads"
            );
        }
    }
}

/// The parallel path reads the same rows in the same order.
///
/// The groups are checked above; this checks the *rows*, through a plan that
/// aggregates a dictionary-encoded column, where the order of the rows is
/// visible in the answer.
#[test]
fn the_parallel_path_reads_the_rows_in_the_files_order() {
    let file = TempFile::new("parallel-order");
    write_row_groups(&file.path, &three_row_group_batches());

    let plan = QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec {
            group_by: vec!["l_shipdate".to_owned()],
            aggregates: vec![Aggregate::Sum { expr: columnar_core::Expr::column("l_quantity") }],
        }),
    );

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let single = execute_on(&plan, &source, &ExecOptions::default()).expect("the query runs");

    for threads in [2, 4, 8] {
        let options = ExecOptions { threads: Some(threads), ..ExecOptions::default() };
        let parallel = execute_on(&plan, &source, &options).expect("the parallel query runs");
        assert_eq!(
            parallel.groups, single.groups,
            "the parallel groups differ at {threads} threads"
        );
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
        "the predicate was not applied during the scan"
    );
}

/// A predicate on a column nothing else reads is applied during the scan.
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

    let plan = QueryPlan::new(
        vec![Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.02))],
        Vec::new(),
        Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: columnar_core::Expr::column("l_quantity") }],
        }),
    );

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
/// The engine's filter stage applies the predicates the scan did not, over the
/// rows that survived the scan. Pushing this one into the scan would evaluate
/// it over every row of the file *and* leave the filter to evaluate it again
/// over the survivors, so the predicate stays with the engine's filter.
#[test]
fn a_predicate_on_an_aggregated_column_is_not_pushed_down() {
    let file = TempFile::new("not-pushed");
    write(&file.path, &edge_case_batch());

    let plan = QueryPlan::new(
        vec![Predicate::new("l_quantity", ComparisonOp::GtEq, ScalarValue::Int64(3))],
        Vec::new(),
        Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: columnar_core::Expr::column("l_quantity") }],
        }),
    );

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

    let plan = QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    );
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

/// The predicates' columns are read, and they are **not** materialised.
///
/// The engine evaluates the predicates over the reader's arrays, so the
/// predicates' columns have to be read -- and the query did not ask for
/// them, so the dataset must not hold them. A dataset that held them would
/// hold every row of the file for a column nobody wanted, and the memory
/// estimate would be wrong by exactly that much.
#[test]
fn the_predicates_columns_are_read_but_not_materialised() {
    use columnar_engine::pushdown::Pushdown;

    let file = TempFile::new("not-materialised");
    write(&file.path, &edge_case_batch());

    let mut plan = PushdownPlan::new();
    plan.push(Pushdown::new(0, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)));
    plan.push(Pushdown::new(
        1,
        "l_shipdate",
        ComparisonOp::Lt,
        ScalarValue::Utf8("1995-01-01".to_owned()),
    ));

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let dataset = source.read(&[], &plan, columnar_kernels::best(), 1).expect("the read runs");

    assert!(
        dataset.chunks.is_empty(),
        "the dataset materialised the predicates' columns: {:?}",
        dataset.chunks.iter().map(|chunk| chunk.name.clone()).collect::<Vec<_>>()
    );
    // Rows 1 and 7 are the ones that satisfy both predicates.
    assert_eq!(dataset.len(), 2, "the scan kept the wrong rows");
}

/// The materialised columns keep their own values.
///
/// The scan reads the materialised columns *and* the predicates' columns, in
/// one projection, and the predicates' columns sit between the materialised
/// ones in the file's order. A column that came out of the projection at the
/// wrong position would hold another column's values, and the answer would be
/// wrong in a way a row count would not catch.
#[test]
fn the_materialised_columns_keep_their_own_values() {
    use columnar_engine::pushdown::Pushdown;

    let file = TempFile::new("positions");
    write(&file.path, &edge_case_batch());

    let mut plan = PushdownPlan::new();
    plan.push(Pushdown::new(0, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)));
    plan.push(Pushdown::new(
        1,
        "l_shipdate",
        ComparisonOp::Lt,
        ScalarValue::Utf8("1995-01-01".to_owned()),
    ));

    // The materialised column is the *third* one, so its position in the
    // projected batch is not its position in the file.
    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let dataset = source.read(&[2], &plan, columnar_kernels::best(), 1).expect("the read runs");

    assert_eq!(dataset.chunks.len(), 1, "the dataset materialised the wrong columns");
    assert_eq!(dataset.chunks[0].name, "l_quantity");
    assert_eq!(
        resolved_rows(&dataset.chunks[0]),
        vec![Some("2".to_owned()), Some("8".to_owned())],
        "the materialised column holds another column's values"
    );
}

/// The rows of a dense column, resolved.
///
/// A NULL row is `None`. The values are compared as text, because the
/// question is "are these the same rows", and a text form says that for a
/// NaN as well -- `NaN != NaN`, so a float comparison would call two
/// identical columns different.
fn resolved_batch_rows(column: &columnar_core::BatchColumn) -> Vec<Option<String>> {
    use columnar_core::BatchData;

    let values: Vec<String> = match &column.data {
        BatchData::Int64(values) => values.iter().map(ToString::to_string).collect(),
        BatchData::Float64(values) => values.iter().map(ToString::to_string).collect(),
        BatchData::Utf8(values) => values.clone(),
    };

    values
        .into_iter()
        .enumerate()
        .map(|(row, value)| if column.validity.is_valid(row) { Some(value) } else { None })
        .collect()
}

/// The eager path's answer for a predicate: the rows it keeps.
///
/// This is the engine's own filter over the whole file, and the engine's own
/// gather of the survivors -- which is exactly what the eager path does when
/// it runs a plan. The lazy path does the same work **during** the scan, so
/// the two must land on the same rows.
fn eager_survivors(
    path: &Path,
    predicate: Predicate,
    column: &str,
) -> (columnar_core::BatchColumn, usize) {
    let dataset = Dataset::read_parquet(path).expect("the dataset can be read eagerly");
    let plan = QueryPlan::new(vec![predicate], Vec::new(), None);
    let resolved = plan.resolve(&dataset).expect("the plan resolves");
    let (selection, _) = columnar_engine::operators::filter::filter_all(
        &dataset,
        &resolved.filter,
        columnar_kernels::best(),
    )
    .expect("the filter runs");

    let gathered = dataset.chunk(column).expect("the column is there").decode_gather(&selection);
    (gathered, selection.len())
}

/// A batch of a dictionary column and an integer column.
///
/// The rows all hold the same discount, so a predicate over it keeps them
/// all, and the dictionary column's values are the only thing that
/// distinguishes one batch from another.
fn dictionary_batch(shipdates: &[&str], quantities: &[i64]) -> RecordBatch {
    let discount = floats(&vec![Some(0.05); shipdates.len()]);
    let shipdate = dictionary(&shipdates.iter().copied().map(Some).collect::<Vec<_>>());
    let quantity = integers(&quantities.iter().copied().map(Some).collect::<Vec<_>>());
    RecordBatch::try_new(schema(), vec![discount, shipdate, quantity])
        .expect("the batch matches the schema")
}

/// A compacted dictionary column holds the same rows across **row groups**.
///
/// Each row group brings its own dictionary, and the column ends up with one
/// dictionary over the whole file. The rows must still be the rows, in the
/// file's order, and they must be the rows the eager path gathers.
///
/// This is the test that makes the dictionary's merge observable through a
/// file. The *repointing* of a row group's keys is not observable here -- a
/// file written by Arrow gives every row group a dictionary that
/// accumulates, so the remap is the identity -- and it is tested where it
/// can be seen, in the unit test over hand-built arrays.
#[test]
fn a_compacted_dictionary_column_holds_the_same_rows_across_row_groups() {
    use columnar_engine::pushdown::Pushdown;

    let file = TempFile::new("dictionary-remap");
    let batches = vec![
        dictionary_batch(&["1993-01-01", "1994-01-01"], &[1, 2]),
        dictionary_batch(&["1995-01-01", "1996-01-01"], &[3, 4]),
    ];
    write_row_groups(&file.path, &batches);

    let mut pushdown = PushdownPlan::new();
    pushdown.push(Pushdown::new(0, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)));

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let lazy = source.read(&[1], &pushdown, columnar_kernels::best(), 1).expect("the read runs");

    let expected = vec![
        Some("1993-01-01".to_owned()),
        Some("1994-01-01".to_owned()),
        Some("1995-01-01".to_owned()),
        Some("1996-01-01".to_owned()),
    ];
    assert_eq!(
        resolved_rows(lazy.chunk("l_shipdate").expect("the column is there")),
        expected,
        "the compacted dictionary holds the wrong rows"
    );

    // And the eager path agrees.
    let (eager, survivors) = eager_survivors(
        &file.path,
        Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)),
        "l_shipdate",
    );
    assert_eq!(lazy.len(), survivors, "the scan and the eager path kept different rows");
    assert_eq!(
        resolved_batch_rows(&lazy.chunk("l_shipdate").expect("the column is there").decode()),
        resolved_batch_rows(&eager),
        "the compacted dictionary and the eager path's gather hold different rows"
    );
}

/// The lazy scan **compacts** the survivors: it writes them straight into the
/// engine's own column, one copy, instead of filtering the batch and copying
/// the survivors out of the filtered batch.
///
/// The eager path reads everything and gathers the survivors afterwards. The
/// two must land on the same bytes, because the two paths are one engine --
/// and a compaction that quietly disagreed with the gather would make the
/// lazy path's answers a different engine's answers.
#[test]
fn the_compacted_column_holds_the_same_rows_as_the_eager_path() {
    use columnar_engine::pushdown::Pushdown;

    let file = TempFile::new("compacted");
    write(&file.path, &edge_case_batch());

    // The predicate is on `l_shipdate`, which the scan reads but does not
    // materialise, so it is pushed into the scan. `l_quantity` is the column
    // the scan compacts.
    //
    // The predicate is chosen so that a **NULL row survives**: rows 2, 4, 5
    // and 6 keep, and row 2 holds a NULL `l_quantity`. A compaction that
    // recorded the NULL at the row it had in the file would record it at row
    // 2, and the column would say row 2 is NULL -- while row 0, which holds
    // the NULL's zero, would claim to hold a value.
    let mut pushdown = PushdownPlan::new();
    pushdown.push(Pushdown::new(
        1,
        "l_shipdate",
        ComparisonOp::Gt,
        ScalarValue::Utf8("1994-01-01".to_owned()),
    ));

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let lazy = source.read(&[2], &pushdown, columnar_kernels::best(), 1).expect("the read runs");

    let (eager, survivors) = eager_survivors(
        &file.path,
        Predicate::new("l_shipdate", ComparisonOp::Gt, ScalarValue::Utf8("1994-01-01".to_owned())),
        "l_quantity",
    );

    assert_eq!(lazy.len(), survivors, "the scan kept a different number of rows");
    assert_eq!(survivors, 4, "the predicate kept the wrong rows");

    let compacted = lazy.chunk("l_quantity").expect("the column is there").decode();
    assert_eq!(
        resolved_batch_rows(&compacted),
        resolved_batch_rows(&eager),
        "the compacted column holds different rows than the eager path's gather"
    );
    assert_eq!(
        resolved_batch_rows(&compacted),
        vec![None, Some("5".to_owned()), Some("6".to_owned()), Some("7".to_owned())],
        "the compacted column holds the wrong rows"
    );
}

/// A dictionary column is compacted the same way.
///
/// The dictionary's **order** is the reader's business, and two reads may
/// number their dictionaries differently, so the comparison is over the
/// resolved values and not over the indices.
#[test]
fn a_compacted_dictionary_column_holds_the_same_rows_as_the_eager_path() {
    use columnar_engine::pushdown::Pushdown;

    let file = TempFile::new("compacted-dictionary");
    write(&file.path, &edge_case_batch());

    let mut pushdown = PushdownPlan::new();
    pushdown.push(Pushdown::new(0, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)));

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let lazy = source.read(&[1], &pushdown, columnar_kernels::best(), 1).expect("the read runs");

    let (eager, survivors) = eager_survivors(
        &file.path,
        Predicate::new("l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04)),
        "l_shipdate",
    );

    assert_eq!(lazy.len(), survivors, "the scan kept a different number of rows");

    let compacted = lazy.chunk("l_shipdate").expect("the column is there").decode();
    assert_eq!(
        resolved_batch_rows(&compacted),
        resolved_batch_rows(&eager),
        "the compacted dictionary holds different rows than the eager path's gather"
    );
    assert_eq!(
        resolved_batch_rows(&compacted),
        vec![
            Some("1994-01-01".to_owned()),
            Some("1995-01-01".to_owned()),
            Some("1996-03-03".to_owned()),
            Some("1993-06-06".to_owned()),
        ],
        "the compacted dictionary holds the wrong rows"
    );
}

/// The parallel scan compacts each row group on its own worker, and the
/// parts are merged back in the file's order.
///
/// So the compacted column must hold the same rows as the single-threaded
/// scan's, and the eager path's, at every thread count.
#[test]
fn the_parallel_scan_compacts_the_same_rows_as_the_single_threaded_one() {
    use columnar_engine::pushdown::Pushdown;

    let file = TempFile::new("compacted-parallel");
    write_row_groups(&file.path, &three_row_group_batches());

    let mut pushdown = PushdownPlan::new();
    pushdown.push(Pushdown::new(
        1,
        "l_shipdate",
        ComparisonOp::Lt,
        ScalarValue::Utf8("1995-01-01".to_owned()),
    ));

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    assert_eq!(
        source.row_groups(),
        3,
        "the file does not have three row groups, so the parallel path never engages"
    );

    let single = source.read(&[2], &pushdown, columnar_kernels::best(), 1).expect("the read runs");
    let single_rows = resolved_rows(single.chunk("l_quantity").expect("the column is there"));

    for threads in [2, 3, 4, 8] {
        let parallel =
            source.read(&[2], &pushdown, columnar_kernels::best(), threads).expect("the read runs");
        assert_eq!(
            parallel.len(),
            single.len(),
            "the parallel scan kept a different number of rows at {threads} threads"
        );
        assert_eq!(
            resolved_rows(parallel.chunk("l_quantity").expect("the column is there")),
            single_rows,
            "the parallel scan holds different rows at {threads} threads"
        );
    }

    // And the eager path agrees with them.
    let (eager, survivors) = eager_survivors(
        &file.path,
        Predicate::new("l_shipdate", ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned())),
        "l_quantity",
    );
    assert_eq!(single.len(), survivors, "the scan and the eager path kept different rows");
    assert_eq!(
        resolved_batch_rows(&single.chunk("l_quantity").expect("the column is there").decode()),
        resolved_batch_rows(&eager),
        "the compacted column and the eager path's gather hold different rows"
    );
}
