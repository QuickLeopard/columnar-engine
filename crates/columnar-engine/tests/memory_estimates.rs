//! The memory estimates.
//!
//! Every public estimate in `memory.rs` is tested here, and every test
//! asserts a **property** rather than a number: a global aggregate's
//! estimate does not grow with the row count, the build side's estimate is
//! at least the space the table really needs, a filtered plan costs no more
//! than an unfiltered one, and no estimate overflows.
//!
//! The data is a small synthetic Parquet file, written by the test itself:
//!
//! | column | type | values |
//! |---|---|---|
//! | `k` | `int64` | `0 .. 1000`, one a row |
//! | `d` | `utf8` | ten dates, `2020-01-01` to `2020-01-10`, a hundred rows a date |
//! | `v` | `float64` | `0.0 .. 1000.0` |
//!
//! The file is written as **ten row groups**, one a date, with the dates in
//! order. That is what makes the file's statistics a **bound** rather than a
//! hint: a row group whose own dates cannot satisfy a range over them holds
//! no row that can satisfy it, so its rows are not counted.

#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Float64Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Predicate, ScalarValue};
use columnar_engine::memory::{
    direct_table_fits, estimate_build_side, estimate_bytes, estimate_lazy_read, estimate_positions,
    estimate_resolved, estimate_resolved_at, estimate_sort, DIRECT_TABLE_LIMIT,
};
use columnar_engine::{
    Aggregate, AggregateSpec, Dataset, JoinSpec, ParquetSource, QueryPlan, SortSpec,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

/// The rows the generated file holds.
const ROWS: usize = 1_000;

/// The days the generated file's date column spans.
const DAYS: usize = 10;

/// The bytes one aggregate hash-table entry takes.
const GROUP_ENTRY_BYTES: usize = 16;

/// The bytes one build-side hash-table row takes.
const HASH_ENTRY_BYTES: usize = 48;

/// The bytes one row position takes.
const POSITION_BYTES: usize = 4;

/// The bytes the direct table's overflow map takes: one entry in every 256
/// of the rows that survive.
fn overflow_bytes(survivors: usize) -> usize {
    survivors.div_ceil(256) * HASH_ENTRY_BYTES
}

/// The bytes one row of the sort holds.
const SORT_ENTRY_BYTES: usize = 16;

/// A path for a generated file, in the system's temporary directory.
fn file_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("columnar-engine-memory-{name}.parquet"))
}

/// The rows one row group holds.
const GROUP_ROWS: usize = ROWS / DAYS;

/// Writes the synthetic file: ten row groups, one a date, in date order.
fn write_file(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("k", DataType::Int64, false),
        Field::new("d", DataType::Utf8, false),
        Field::new("v", DataType::Float64, false),
    ]));

    let file = File::create(path).expect("the file is created");
    // The writer buffers rows and flushes a row group when it reaches the
    // limit, so the limit is what decides how many row groups the file has.
    let properties = WriterProperties::builder().set_max_row_group_size(GROUP_ROWS).build();
    let mut writer =
        ArrowWriter::try_new(file, schema.clone(), Some(properties)).expect("the writer is built");
    for group in 0..DAYS {
        let first = group * GROUP_ROWS;
        let keys: Vec<i64> = (first..first + GROUP_ROWS)
            .map(|row| i64::try_from(row).expect("the row count fits in an i64"))
            .collect();
        let dates: Vec<String> =
            std::iter::repeat_n(format!("2020-01-{:02}", group + 1), GROUP_ROWS).collect();
        let values: Vec<f64> = (first..first + GROUP_ROWS)
            .map(|row| f64::from(i32::try_from(row).expect("the row count fits in an i32")))
            .collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(keys)),
                Arc::new(StringArray::from(dates)),
                Arc::new(Float64Array::from(values)),
            ],
        )
        .expect("the batch is built");

        writer.write(&batch).expect("the batch is written");
    }
    writer.close().expect("the file is closed");
}

/// The synthetic file, opened as a source.
fn source(name: &str) -> ParquetSource {
    let path = file_path(name);
    write_file(&path);
    ParquetSource::open(&path).expect("the source opens")
}

/// The synthetic file, read as a dataset.
fn dataset(name: &str) -> Dataset {
    let path = file_path(name);
    write_file(&path);
    Dataset::read_parquet(&path).expect("the dataset is read")
}

/// A plan that counts every row: one group, whatever the row count is.
fn global_count() -> QueryPlan {
    QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// A plan that counts the rows of each key: a group a key.
fn grouped_count() -> QueryPlan {
    QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec {
            group_by: vec!["k".to_owned()],
            aggregates: vec![Aggregate::CountStar],
        }),
    )
}

/// A filter over the date column, selecting two of the file's ten days.
fn date_filter() -> Vec<Predicate> {
    vec![
        Predicate::new("d", ComparisonOp::GtEq, "2020-01-03"),
        Predicate::new("d", ComparisonOp::Lt, "2020-01-05"),
    ]
}

/// The rows that really survive the date filter: two days a hundred rows.
const REAL_SURVIVORS: usize = ROWS * 2 / DAYS;

#[test]
fn the_column_range_is_the_file_own() {
    let source = source("range");

    let (min, max) = source.column_range("k").expect("the key column has statistics");
    assert_eq!(min, ScalarValue::Int64(0));
    assert_eq!(max, ScalarValue::Int64(i64::try_from(ROWS - 1).expect("the row count fits")));

    let (min, max) = source.column_range("d").expect("the date column has statistics");
    assert_eq!(min, ScalarValue::Utf8("2020-01-01".to_owned()));
    assert_eq!(max, ScalarValue::Utf8("2020-01-10".to_owned()));

    assert!(source.column_range("nope").is_none(), "a column the file does not have has no range");
}

#[test]
fn a_global_aggregate_does_not_cost_more_as_the_rows_grow() {
    let dataset = dataset("global");
    let resolved = global_count().resolve(&dataset).expect("the plan resolves");

    let small = estimate_resolved_at(&resolved, &dataset, 10, false);
    let large = estimate_resolved_at(&resolved, &dataset, 10_000, false);

    // The aggregate holds one group, so the only part of the estimate that
    // grows with the row count is the filter's mask: two bits a row.
    let mask = |rows: usize| rows.div_ceil(8) * 2;
    assert_eq!(large - small, mask(10_000) - mask(10), "the groups grew with the rows");
}

#[test]
fn a_grouped_aggregate_costs_a_group_a_row() {
    let dataset = dataset("grouped");
    let resolved = global_count().resolve(&dataset).expect("the plan resolves");

    let rows = 1_000;
    let global = estimate_resolved_at(&resolved, &dataset, rows, false);
    let grouped = estimate_resolved_at(&resolved, &dataset, rows, true);

    // One group for the global aggregate, a group a row for the grouped one.
    assert_eq!(grouped - global, (rows - 1) * GROUP_ENTRY_BYTES);
}

#[test]
fn a_plan_that_groups_is_charged_for_its_groups_whatever_the_flag_says() {
    let dataset = dataset("grouped-plan");
    let resolved = grouped_count().resolve(&dataset).expect("the plan resolves");

    let rows = 1_000;
    // The plan groups by a column, so it can have as many groups as it has
    // rows. The flag can only make the estimate larger, never smaller.
    assert_eq!(
        estimate_resolved_at(&resolved, &dataset, rows, false),
        estimate_resolved_at(&resolved, &dataset, rows, true)
    );
}

#[test]
fn a_filtered_plan_costs_no_more_than_an_unfiltered_one() {
    let dataset = dataset("filtered-plan");
    let unfiltered = QueryPlan::new(Vec::new(), vec!["k".to_owned()], None);
    let filtered = QueryPlan::new(date_filter(), vec!["k".to_owned()], None);

    let plain = estimate_bytes(&unfiltered, &dataset);
    let narrowed = estimate_bytes(&filtered, &dataset);
    assert!(narrowed <= plain, "a filter cannot make a plan cost more");
}

#[test]
fn the_build_side_estimate_is_at_least_the_space_the_table_really_needs() {
    let source = source("build-side");
    let filter = date_filter();

    // The key is an integer in a range that fits, so the join builds the
    // direct-addressed table: one byte a key, over the key's own range.
    let estimate = estimate_build_side(&source, "k", &filter, 1);
    assert!(estimate >= ROWS, "the estimate is below the direct-addressed table");

    // The dataset the read holds: the file's rows, over the key's width and
    // the validity bitmap. The peak is during the read, when every row of
    // the file is still in flight.
    assert!(estimate >= ROWS * 8 + ROWS.div_ceil(8), "the estimate is below the file's rows");
}

#[test]
fn the_build_side_estimate_is_charged_the_read_the_join_reads() {
    let source = source("build-read");
    let filter = date_filter();

    // The read set is the **reader's** decision, and the estimate asks the
    // reader the same question the join asks: the key, and every column of a
    // predicate the reader declined. The estimate cannot charge a different
    // set, because it never writes the rules out a second time.
    //
    // The file's rows over the key's width and the validity bitmap, the keys
    // the join collects out of the rows that survive, and the table it builds
    // over them.
    let dataset = ROWS * 8 + ROWS.div_ceil(8);
    let keys = REAL_SURVIVORS * 8;
    let table = ROWS + overflow_bytes(REAL_SURVIVORS);
    assert_eq!(estimate_build_side(&source, "k", &filter, 1), dataset + keys + table);
}

#[test]
fn a_parallel_read_is_charged_two_copies() {
    let source = source("build-copies");
    let filter = date_filter();

    // One thread decodes straight into the dataset and holds one copy of it.
    // A parallel read holds every part at once and then the merged result, so
    // it holds **two** -- and the estimate charges the peak, not the result.
    let one = estimate_build_side(&source, "k", &filter, 1);
    let many = estimate_build_side(&source, "k", &filter, 8);
    assert!(many > one, "the parallel read is not charged its peak");
    assert_eq!(many - one, ROWS * 8 + ROWS.div_ceil(8));
}

#[test]
fn a_predicate_the_reader_declined_is_charged_the_filters_own_mask() {
    let source = source("build-declined");

    // A predicate over the **key** is declined: the key is already being
    // read, so the engine's filter applies it over the rows that survived,
    // and the filter holds a mask and a selection vector over them.
    let on_the_key = vec![Predicate::new("k", ComparisonOp::GtEq, 0_i64)];
    let estimate = estimate_build_side(&source, "k", &on_the_key, 1);
    let without = estimate_build_side(&source, "k", &[], 1);
    assert_eq!(estimate - without, ROWS.div_ceil(8) * 2);
}

#[test]
fn a_build_side_whose_key_cannot_be_addressed_is_charged_the_hash_table() {
    let source = source("build-hash");

    // A float key cannot be addressed by its own value, so the join builds a
    // hash table over the rows that survive the filter: a row a surviving
    // row.
    let estimate = estimate_build_side(&source, "v", &date_filter(), 1);
    assert!(
        estimate >= REAL_SURVIVORS * HASH_ENTRY_BYTES,
        "the estimate is below the hash table the join may build"
    );
}

#[test]
fn the_build_side_estimate_covers_the_worst_case() {
    let source = source("build-worst");

    // Without a filter every row of the file can survive, and the hash table
    // over every row is the largest table the join may build. The key is a
    // float here, so that is the table the join builds.
    let estimate = estimate_build_side(&source, "v", &[], 1);
    assert!(
        estimate >= ROWS * HASH_ENTRY_BYTES,
        "the estimate is below the table the worst case needs"
    );
}

#[test]
fn a_filtered_build_side_costs_no_more_than_an_unfiltered_one() {
    let source = source("build-filtered");
    let filter = date_filter();

    let unfiltered = estimate_build_side(&source, "k", &[], 1);
    let filtered = estimate_build_side(&source, "k", &filter, 1);

    assert!(filtered <= unfiltered, "a filter cannot make the build side cost more");
    assert!(filtered < unfiltered, "the filter does not narrow the estimate");
}

#[test]
fn the_build_side_bound_is_exactly_the_row_groups_that_can_survive() {
    let source = source("build-groups");
    let filter = date_filter();

    // The file has ten row groups, one a date, and the filter selects two of
    // the days. The two row groups that hold those days are counted, and the
    // eight that cannot satisfy the filter are not.
    let groups = source.row_group_ranges("d");
    assert_eq!(groups.len(), DAYS, "the file has a row group a date");
    for group in groups {
        assert_eq!(group.rows, GROUP_ROWS, "a row group holds a hundred rows");
    }

    // The estimate is the file's rows over the key's width and the validity
    // bitmap, the keys the join collects out of the rows that survive, and
    // the table the join will build: the direct-addressed one, a byte a key
    // over the key's own range, and the overflow map beside it.
    let dataset = ROWS * 8 + ROWS.div_ceil(8);
    let keys = REAL_SURVIVORS * 8;
    let table = ROWS + overflow_bytes(REAL_SURVIVORS);
    assert_eq!(estimate_build_side(&source, "k", &filter, 1), dataset + keys + table);
}

#[test]
fn a_row_group_that_cannot_satisfy_the_filter_is_not_counted() {
    let source = source("build-groups-narrow");

    // A filter over a day the file's first row group does not hold excludes
    // that group, and the estimate falls by its rows.
    let wide = vec![Predicate::new("d", ComparisonOp::GtEq, "2020-01-01")];
    let narrow = vec![Predicate::new("d", ComparisonOp::GtEq, "2020-01-06")];
    let wide_estimate = estimate_build_side(&source, "k", &wide, 1);
    let narrow_estimate = estimate_build_side(&source, "k", &narrow, 1);
    assert!(narrow_estimate < wide_estimate, "the narrower filter does not cost less");
}

#[test]
fn a_build_side_the_statistics_cannot_bound_costs_every_row() {
    let source = source("build-no-stats");

    // A filter on a column the file has no statistics for cannot narrow the
    // estimate, so it falls back to every row.
    let unknown = vec![Predicate::new("nope", ComparisonOp::GtEq, 1_i64)];
    let unfiltered = estimate_build_side(&source, "k", &[], 1);
    assert_eq!(estimate_build_side(&source, "k", &unknown, 1), unfiltered);

    // A column whose values have no place on a line -- a float, say -- is no
    // better: its statistics cannot say how many rows a range lets through.
    let float = vec![Predicate::new("v", ComparisonOp::GtEq, 1.0_f64)];
    assert_eq!(estimate_build_side(&source, "k", &float, 1), unfiltered);
}

#[test]
fn the_build_side_charges_the_table_the_join_will_build() {
    let source = source("build-tables");

    // The key column's range fits the budget, so the join builds the
    // direct-addressed table: one byte a key, over the key's own range.
    assert!(direct_table_fits(u64::try_from(ROWS).expect("the row count fits")));
    let estimate = estimate_build_side(&source, "k", &[], 1);
    assert_eq!(estimate, ROWS * 8 + ROWS.div_ceil(8) + ROWS * 8 + ROWS + overflow_bytes(ROWS));

    // A key that cannot be addressed by its own value is charged the hash
    // table over the rows that survive -- and its keys are collected into
    // the join's own key type, which is wider than an integer.
    let hashed = estimate_build_side(&source, "v", &[], 1);
    assert_eq!(hashed, ROWS * 8 + ROWS.div_ceil(8) + ROWS * 32 + ROWS * HASH_ENTRY_BYTES);
}

#[test]
fn a_direct_table_fits_up_to_the_limit() {
    let limit = u64::try_from(DIRECT_TABLE_LIMIT).expect("the limit fits in a u64");
    assert!(direct_table_fits(limit));
    assert!(direct_table_fits(0));
    assert!(!direct_table_fits(limit + 1));
    assert!(!direct_table_fits(u64::MAX));
}

#[test]
fn the_lazy_estimate_of_a_global_aggregate_does_not_grow_with_the_rows() {
    let source = source("lazy-global");

    let global = estimate_lazy_read(&source, &[0], 1, false);
    let grouped = estimate_lazy_read(&source, &[0], 1, true);

    // One group for the global aggregate, a group a row for the grouped one.
    assert_eq!(grouped - global, (source.rows() - 1) * GROUP_ENTRY_BYTES);
}

#[test]
fn a_parallel_read_costs_more_than_a_single_one() {
    let source = source("lazy-parallel");

    let one = estimate_lazy_read(&source, &[0, 1], 1, false);
    let many = estimate_lazy_read(&source, &[0, 1], 4, false);
    assert!(many > one, "the parallel read holds two copies of the read set");
}

#[test]
fn reading_more_columns_costs_more() {
    let source = source("lazy-columns");

    let one = estimate_lazy_read(&source, &[0], 1, false);
    let two = estimate_lazy_read(&source, &[0, 1], 1, false);
    assert!(two > one, "a column the reader fetches is memory the engine holds");
}

#[test]
fn a_sort_costs_the_rows_it_sorts() {
    let rows = 1_000;

    let bounded = estimate_sort(rows, Some(10));
    let every = estimate_sort(rows, None);
    assert_eq!(bounded, 10 * SORT_ENTRY_BYTES);
    assert_eq!(every, rows * SORT_ENTRY_BYTES);
    assert!(bounded < every, "a limit bounds the sort's working set");
    assert_eq!(estimate_sort(rows, Some(rows * 10)), every, "a limit over the rows is the rows");
}

#[test]
fn the_row_positions_are_counted() {
    assert_eq!(estimate_positions(ROWS), ROWS * POSITION_BYTES);
}

#[test]
fn a_plan_with_a_sort_carries_the_row_positions() {
    let dataset = dataset("positions-sort");
    let plain = QueryPlan::new(Vec::new(), vec!["k".to_owned()], None);
    let sorted = QueryPlan {
        sort: Some(SortSpec { column: "k".to_owned(), descending: false, limit: None }),
        ..plain.clone()
    };

    let without =
        estimate_resolved(&plain.resolve(&dataset).expect("the plan resolves"), &dataset, false);
    let with =
        estimate_resolved(&sorted.resolve(&dataset).expect("the plan resolves"), &dataset, false);
    assert_eq!(with - without, estimate_positions(dataset.len()));
}

#[test]
fn a_plan_with_a_join_carries_the_row_positions() {
    let dataset = dataset("positions-join");
    let plain = QueryPlan::new(Vec::new(), vec!["k".to_owned()], None);
    let joined = QueryPlan {
        join: Some(JoinSpec {
            build_path: file_path("positions-join"),
            probe_key: "k".to_owned(),
            build_key: "k".to_owned(),
            build_filter: Vec::new(),
        }),
        ..plain.clone()
    };

    let without =
        estimate_resolved(&plain.resolve(&dataset).expect("the plan resolves"), &dataset, false);
    let with =
        estimate_resolved(&joined.resolve(&dataset).expect("the plan resolves"), &dataset, false);
    assert_eq!(with - without, estimate_positions(dataset.len()));
}

#[test]
fn an_unresolvable_plan_has_no_estimate() {
    let dataset = dataset("unresolvable");
    let plan = QueryPlan::new(Vec::new(), vec!["nope".to_owned()], None);
    assert_eq!(estimate_bytes(&plan, &dataset), 0);
}

#[test]
fn no_estimate_overflows() {
    let dataset = dataset("overflow");
    let resolved = grouped_count().resolve(&dataset).expect("the plan resolves");

    assert_eq!(estimate_resolved_at(&resolved, &dataset, usize::MAX, true), usize::MAX);
    assert_eq!(estimate_resolved_at(&resolved, &dataset, usize::MAX, false), usize::MAX);
    assert_eq!(estimate_sort(usize::MAX, None), usize::MAX);
    assert_eq!(estimate_sort(usize::MAX, Some(usize::MAX)), usize::MAX);
    assert_eq!(estimate_positions(usize::MAX), usize::MAX);
}
