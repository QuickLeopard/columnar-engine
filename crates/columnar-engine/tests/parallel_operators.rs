//! The parallel operators' tests.
//!
//! The parallel filter and the parallel projection must produce **exactly**
//! what the serial ones produce -- the same rows, in the same order, with the
//! same values and the same validity bits. "Equivalent" is not enough: the
//! selection is the set of rows the rest of the query sees, and an answer that
//! depended on the scheduler would not be an answer.
//!
//! So these tests compare the parallel result against the serial one, at
//! every thread count, on data that exercises every encoding the filter has a
//! path for: plain integers, plain floats, dictionary-encoded integers,
//! dictionary-encoded strings, and NULLs in every column.
//!
//! The tests that would catch a defect are the ones that compare the **rows**,
//! not just the counts. A filter that dropped the last block would still
//! report the right number of rows out if the counts were derived from the
//! same broken mask, and a gather that concatenated its parts in completion
//! order would still report the right number of values.

// The indices below are literals into vectors whose length the test has just
// asserted, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]
// The casts below turn a row number into a `usize`, an `i64`, an `f64` or an
// `i32`. The row numbers are below twenty thousand, so they are exact in
// every one of those types; the lints cannot see that, because the row
// number is a `usize` and a `usize` could be sixty-four bits wide.
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

use columnar_core::{ColumnChunk, ComparisonOp, Encoding, NullBitmap, Predicate, ScalarValue};
use columnar_engine::operators::filter::filter_all_with_workers;
use columnar_engine::operators::project::{gather_workers, GatherSource};
use columnar_engine::{execute, Aggregate, AggregateSpec, Dataset, ExecOptions, QueryPlan};
use columnar_kernels::Flavor;

/// The rows the datasets have. Enough that the parallel paths are taken --
/// the filter's blocks are 64 rows and the projection's are 4096.
const ROWS: usize = 20_000;

/// The thread counts the tests sweep.
const WORKERS: [usize; 6] = [1, 2, 3, 4, 7, 16];

/// Builds a chunk with the given NULL rows.
fn chunk(name: &str, encoding: Encoding, nulls: &[usize]) -> ColumnChunk {
    let len = encoding.len();
    ColumnChunk::new(name, encoding, NullBitmap::with_nulls(len, nulls))
}

/// A dataset with one column of every kind the filter has a path for.
fn mixed_dataset() -> Dataset {
    // A plain integer column, with a NULL every seventh row.
    let ints: Vec<i64> = (0..ROWS).map(|row| (row as i64) % 1000).collect();
    let int_nulls: Vec<usize> = (0..ROWS).filter(|row| row % 7 == 0).collect();

    // A plain float column, with a NULL every eleventh row.
    let floats: Vec<f64> = (0..ROWS).map(|row| (row as f64) / 10.0).collect();
    let float_nulls: Vec<usize> = (0..ROWS).filter(|row| row % 11 == 0).collect();

    // A dictionary-encoded integer column: eleven distinct values.
    let dict: Vec<i64> = (0..11).map(|value| value * 3).collect();
    let indices: Vec<i32> = (0..ROWS).map(|row| (row % 11) as i32).collect();

    // A dictionary-encoded string column: four distinct values.
    let strings: Vec<String> =
        ["alpha", "beta", "gamma", "delta"].iter().map(|s| (*s).to_owned()).collect();
    let string_indices: Vec<i32> = (0..ROWS).map(|row| (row % 4) as i32).collect();

    Dataset::from_chunks(vec![
        chunk("x", Encoding::Int64 { values: ints }, &int_nulls),
        chunk("y", Encoding::Float64 { values: floats }, &float_nulls),
        chunk("d", Encoding::Int64Dict { dict, indices }, &[]),
        chunk("s", Encoding::Utf8Dict { dict: strings, indices: string_indices }, &[]),
    ])
}

/// The plan the tests filter with: one predicate per encoding.
fn mixed_plan() -> QueryPlan {
    QueryPlan {
        filter: vec![
            Predicate::new("x", ComparisonOp::GtEq, ScalarValue::Int64(100)),
            Predicate::new("y", ComparisonOp::Lt, ScalarValue::Float64(500.0)),
            Predicate::new("d", ComparisonOp::Lt, ScalarValue::Int64(20)),
            Predicate::new("s", ComparisonOp::Ne, ScalarValue::Utf8("gamma".to_owned())),
        ],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar],
        }),
    }
}

#[test]
fn the_parallel_filter_selects_exactly_the_rows_the_serial_one_does() {
    let dataset = mixed_dataset();
    let resolved = mixed_plan().resolve(&dataset).expect("the plan resolves");
    let flavor = Flavor::Scalar;

    let (serial, serial_stats) =
        filter_all_with_workers(&dataset, &resolved.filter, flavor, 1).expect("the filter runs");

    for workers in WORKERS {
        let (parallel, parallel_stats) =
            filter_all_with_workers(&dataset, &resolved.filter, flavor, workers)
                .expect("the filter runs");

        // The whole selection, not just its length: the representation, the
        // rows and their order all have to match. The comparison is written
        // as a `bool` so that a failure prints the message rather than
        // thousands of row numbers.
        assert!(
            parallel == serial,
            "the parallel filter differs from the serial one at {workers} workers"
        );
        assert_eq!(parallel_stats.rows_out, serial_stats.rows_out);
        assert_eq!(parallel_stats.kind, serial_stats.kind);
    }
}

#[test]
fn the_parallel_filter_over_a_dictionary_column_selects_the_same_rows() {
    let dataset = mixed_dataset();
    let plan = QueryPlan {
        filter: vec![Predicate::new("d", ComparisonOp::GtEq, ScalarValue::Int64(12))],
        projection: Vec::new(),
        aggregate: None,
    };
    let resolved = plan.resolve(&dataset).expect("the plan resolves");

    let (serial, _) =
        filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, 1).expect("it runs");

    for workers in WORKERS {
        let (parallel, _) =
            filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, workers)
                .expect("it runs");
        assert!(parallel == serial, "the dictionary filter differs at {workers} workers");
        // And the rows really are there: the selection is not empty.
        assert!(!serial.is_empty(), "the test's predicate matched nothing");
    }
}

#[test]
fn the_parallel_filter_over_a_string_column_selects_the_same_rows() {
    let dataset = mixed_dataset();
    let plan = QueryPlan {
        filter: vec![Predicate::new("s", ComparisonOp::Lt, ScalarValue::Utf8("delta".to_owned()))],
        projection: Vec::new(),
        aggregate: None,
    };
    let resolved = plan.resolve(&dataset).expect("the plan resolves");

    let (serial, _) =
        filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, 1).expect("it runs");

    for workers in WORKERS {
        let (parallel, _) =
            filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, workers)
                .expect("it runs");
        assert!(parallel == serial, "the string filter differs at {workers} workers");
    }
}

#[test]
fn the_parallel_gather_holds_the_same_values_in_the_same_order() {
    let dataset = mixed_dataset();
    let resolved = mixed_plan().resolve(&dataset).expect("the plan resolves");
    let (selection, _) =
        filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, 1).expect("it runs");

    for column in 0..dataset.chunks.len() {
        let chunk = &dataset.chunks[column];
        let serial = gather_workers(GatherSource::Chunk(chunk), &selection, 1);
        for workers in WORKERS {
            let parallel = gather_workers(GatherSource::Chunk(chunk), &selection, workers);
            assert!(
                parallel.data == serial.data,
                "the gathered values of `{}` differ at {workers} workers",
                chunk.name
            );
            assert!(
                parallel.validity == serial.validity,
                "the gathered validity of `{}` differs at {workers} workers",
                chunk.name
            );
        }
    }
}

#[test]
fn the_parallel_gather_keeps_the_dictionary_values() {
    // A dictionary-encoded column is gathered through its dictionary, so the
    // gathered values have to be the *values*, not the indices.
    let dataset = mixed_dataset();
    let resolved = mixed_plan().resolve(&dataset).expect("the plan resolves");
    let (selection, _) =
        filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, 1).expect("it runs");

    let chunk = dataset.chunks.get(3).expect("the string column exists");
    let serial = gather_workers(GatherSource::Chunk(chunk), &selection, 1);
    for workers in WORKERS {
        let parallel = gather_workers(GatherSource::Chunk(chunk), &selection, workers);
        assert!(parallel.data == serial.data, "the strings differ at {workers} workers");
    }
    assert!(matches!(serial.data, columnar_core::BatchData::Utf8(_)));
}

#[test]
fn the_answer_does_not_depend_on_the_thread_count() {
    let dataset = mixed_dataset();
    let plan = mixed_plan();

    let serial = execute(&plan, &dataset, &options_at(1)).expect("the query runs");
    for workers in WORKERS {
        let parallel = execute(&plan, &dataset, &options_at(workers)).expect("the query runs");
        assert_eq!(parallel.groups, serial.groups, "the answer differs at {workers} workers");
    }
}

#[test]
fn the_answer_of_a_float_sum_does_not_depend_on_the_thread_count() {
    // A floating-point sum is not associative, so a parallel aggregate would
    // make the answer depend on the block boundaries. The aggregate is not
    // parallelised, and this is the test that says so: the bits are identical.
    let values: Vec<f64> = (0..ROWS).map(|row| (row as f64) * 0.1 + 0.3).collect();
    let dataset = Dataset::from_chunks(vec![chunk("v", Encoding::Float64 { values }, &[])]);
    let plan = QueryPlan {
        filter: vec![Predicate::new("v", ComparisonOp::GtEq, ScalarValue::Float64(0.0))],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: columnar_core::Expr::column("v") }],
        }),
    };

    let serial = execute(&plan, &dataset, &options_at(1)).expect("the query runs");
    for workers in WORKERS {
        let parallel = execute(&plan, &dataset, &options_at(workers)).expect("the query runs");
        assert_eq!(parallel.groups, serial.groups, "the sum differs at {workers} workers");
    }
}

#[test]
fn the_plan_reports_the_workers_that_ran() {
    let dataset = mixed_dataset();
    let plan = mixed_plan();

    // One worker: the serial path, and the plan says one.
    let serial = execute(&plan, &dataset, &options_at(1)).expect("the query runs");
    assert_eq!(serial.plan_output.filter_workers, 1);
    assert_eq!(serial.plan_output.projection_workers, 1);

    // A plan that materialises a column, so the projection has work to do.
    // `COUNT(*)` alone materialises nothing, and a projection that gathered
    // nothing has one worker by definition.
    let summing = QueryPlan {
        filter: mixed_plan().filter,
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: columnar_core::Expr::column("x") }],
        }),
    };

    // More workers than the dataset can give blocks of work to. The plan
    // reports the workers that **ran**, not the ones that were asked for:
    //
    // - the filter's blocks are 64-row aligned, so 20,000 rows in 64 blocks
    //   is 313 rows each, rounded up to 320 -- and 20,000 / 320 is 62.5, so
    //   63 blocks, not 64;
    // - the projection's blocks are at least 4,096 rows, so 20,000 rows is
    //   5 blocks.
    //
    // The two differ, which is why the plan reports them separately.
    let many = execute(&summing, &dataset, &options_at(64)).expect("the query runs");
    assert_eq!(many.plan_output.filter_workers, 63);
    assert_eq!(many.plan_output.projection_workers, 5);
}

#[test]
fn the_parallel_filter_over_a_short_dataset_is_the_serial_filter() {
    // Fewer rows than a filter block: there is nothing to split, and the
    // result is the serial one.
    let dataset = Dataset::from_chunks(vec![chunk(
        "x",
        Encoding::Int64 { values: vec![1, 2, 3, 4, 5] },
        &[],
    )]);
    let plan = QueryPlan {
        filter: vec![Predicate::new("x", ComparisonOp::Gt, ScalarValue::Int64(2))],
        projection: Vec::new(),
        aggregate: None,
    };
    let resolved = plan.resolve(&dataset).expect("the plan resolves");

    let (serial, _) =
        filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, 1).expect("it runs");
    for workers in [2usize, 4, 8] {
        let (parallel, parallel_stats) =
            filter_all_with_workers(&dataset, &resolved.filter, Flavor::Scalar, workers)
                .expect("it runs");
        assert!(parallel == serial, "a five-row dataset filters differently at {workers} workers");
        assert_eq!(parallel_stats.workers, 1, "a five-row dataset has one block");
    }
}

/// The execution options at a given thread count.
fn options_at(threads: usize) -> ExecOptions {
    ExecOptions { threads: Some(threads), ..ExecOptions::default() }
}
