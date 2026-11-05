//! The store's table is read by the engine as **one table**.
//!
//! # Why this file exists
//!
//! The store writes a table as a **set of files**, and the engine reads a set of files through
//! one source over a set. **The two halves meet in a list of paths**, and that list is the whole
//! of the glue.
//!
//! **Every test until now checked one half or the other.** The store's tests check what the
//! store writes -- the file the generator wrote, the manifest, the commit, the grid. The
//! engine's tests check what the engine reads from a set of files it was handed. **The
//! composition was checked by hand and not by the suite**, and the composition is the claim the
//! whole store's half rests on: the engine reads what the store writes, as one table.
//!
//! So this file is the composition, and it says four things:
//!
//! - **The rows.** The set holds the table's rows, row by row, in the **files' order** -- the
//!   first file's rows first, and the second file's after them.
//! - **The predicate.** The engine's own predicate reaches **every** file, so the count over the
//!   set is the files' counts added up, and not any one file's.
//! - **The aggregates.** The sum over the set is the files' sums added up -- the values are read,
//!   not merely counted.
//! - **The metadata.** The set's rows are the table's rows, and its row groups are the files'.
//!
//! # What this file costs
//!
//! **The engine is a dev-dependency here, and only here.** The store's *library* depends on
//! nothing in this workspace, and the engine never depends on the store -- the glue is a list of
//! paths, and this file is where the list is handed over.
//!
//! The dependency is **path-only**, and that is not a style choice: a path-only dependency is
//! **dropped** from the published manifest, and a dependency with a version is not. A versioned
//! dev-dependency on the engine would make `cargo package -p columnar-store` wait for the engine
//! to be on crates.io -- the same trap the generator's dev-dependency fell into, and the reason is
//! in ADR-0047.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::{
    execute_on, open_source, Aggregate, AggregateSpec, AggregateValue, ComparisonOp, Dataset,
    ExecOptions, Expr, FileSetSource, Flavor, Predicate, QueryPlan, ScalarValue,
};
use columnar_store::{Store, BATCH_ROWS};

/// The rows in one commit: the writer's grid, exactly.
const ROWS: usize = BATCH_ROWS;

/// The commits in the table: two files, one table.
const COMMITS: usize = 2;

/// The column the rows are compared on.
const KEY_COLUMN: &str = "k";

/// The column the names are read on.
const NAME_COLUMN: &str = "v";

/// A scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("columnar-store-glue-{name}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A batch of rows on the writer's grid, starting at `first`.
fn batch(first: usize) -> RecordBatch {
    let keys: Int64Array = (0..ROWS)
        .map(|row| i64::try_from(first + row).expect("the key fits"))
        .collect::<Vec<_>>()
        .into();
    let names: StringArray =
        (0..ROWS).map(|row| format!("row-{}", first + row)).collect::<Vec<_>>().into();
    let schema = Schema::new(vec![
        Field::new(KEY_COLUMN, DataType::Int64, false),
        Field::new(NAME_COLUMN, DataType::Utf8, false),
    ]);
    RecordBatch::try_new(schema.into(), vec![Arc::new(keys), Arc::new(names)])
        .expect("the batch is built")
}

/// A store holding a table of `COMMITS` commits, and the table's paths.
fn table_of_commits(name: &str) -> (Store, Vec<PathBuf>) {
    let store = Store::open(scratch(name)).expect("the store opens");
    for commit in 0..COMMITS {
        let first = commit * ROWS;
        store.append("t", &[batch(first)]).expect("the rows are written and committed");
    }
    let table = store.table("t").expect("the manifest reads").expect("the table is there");
    (store, table.paths())
}

/// Reads the given columns of a set, through the engine's own reader.
fn read_set(source: &FileSetSource, columns: &[usize]) -> Dataset {
    source.read(columns, &PushdownPlan::new(), Flavor::Scalar, 1).expect("the read runs")
}

/// The count the engine computed, as a row count.
fn counted(result: &columnar_engine::AggregateResult) -> usize {
    match result.groups.first().and_then(|group| group.values.first()) {
        Some(AggregateValue::Int(value)) => {
            usize::try_from(*value).expect("the count is not negative")
        }
        other => panic!("the count is not an integer: {other:?}"),
    }
}

/// The sum the engine computed.
fn summed(result: &columnar_engine::AggregateResult) -> i64 {
    match result.groups.first().and_then(|group| group.values.first()) {
        Some(AggregateValue::Int(value)) => *value,
        other => panic!("the sum is not an integer: {other:?}"),
    }
}

/// A plan that counts the rows a predicate keeps.
fn count_plan(column: &str, op: ComparisonOp, value: ScalarValue) -> QueryPlan {
    QueryPlan::new(
        vec![Predicate::new(column, op, value)],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// A plan that sums a column.
fn sum_plan(column: &str) -> QueryPlan {
    QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column(column) }],
        }),
    )
}

/// **The table the store wrote is the table the engine reads.** The rows are the table's rows,
/// in the files' order, and the set's metadata is the table's.
#[test]
fn the_stores_table_is_read_as_one_table() {
    let (store, paths) = table_of_commits("table");
    let table = store.table("t").expect("the manifest reads").expect("the table is there");

    assert_eq!(table.files().len(), COMMITS, "the table holds one file per commit");
    assert_eq!(
        table.rows(),
        u64::try_from(ROWS * COMMITS).expect("the rows fit"),
        "the table holds every commit's rows"
    );

    let set = FileSetSource::open(&paths).expect("the engine opens the store's table");
    assert_eq!(
        set.rows(),
        usize::try_from(table.rows()).expect("the rows fit"),
        "the set and the table hold different row counts"
    );
    assert_eq!(set.row_groups(), COMMITS, "the set holds one row group per file");

    // The rows, row by row, in the files' order: the first file's rows first.
    let dataset = read_set(&set, &[0, 1]);
    assert_eq!(dataset.len(), ROWS * COMMITS, "the set holds every row");
    let keys = dataset.chunk(KEY_COLUMN).expect("the key column is there");
    for row in 0..dataset.len() {
        let value = match keys.value_at(row) {
            Some(ScalarValue::Int64(value)) => value,
            other => panic!("row {row} is not an integer: {other:?}"),
        };
        assert_eq!(value, i64::try_from(row).expect("the row fits"), "row {row} is out of order");
    }

    // The names come back with the rows.
    let names = dataset.chunk(NAME_COLUMN).expect("the name column is there");
    for row in [0usize, 1, ROWS - 1, ROWS, ROWS * COMMITS - 1] {
        let value = match names.value_at(row) {
            Some(ScalarValue::Utf8(value)) => value,
            other => panic!("row {row} is not a string: {other:?}"),
        };
        assert_eq!(value, format!("row-{row}"), "row {row} holds the wrong name");
    }
}

/// **The engine's own predicate reaches every file**, so the count over the set is the files'
/// counts added up -- and it is not any one file's count.
#[test]
fn the_predicate_reaches_every_file() {
    let (_store, paths) = table_of_commits("predicate");
    let set = FileSetSource::open(&paths).expect("the engine opens the store's table");

    // Every row of the second commit, and none of the first.
    let plan = count_plan(
        KEY_COLUMN,
        ComparisonOp::GtEq,
        ScalarValue::Int64(i64::try_from(ROWS).expect("the rows fit")),
    );

    let over_set =
        set.execute(&plan, &ExecOptions::default()).expect("the query runs over the set");
    let mut over_files = 0usize;
    for path in &paths {
        let one = open_source(path).expect("the file opens");
        let result = execute_on(&plan, &one, &ExecOptions::default()).expect("the query runs");
        over_files += counted(&result);
    }

    assert_eq!(counted(&over_set), over_files, "the set and the files disagree on the count");
    assert_eq!(counted(&over_set), ROWS, "the predicate did not reach the second file");
}

/// **The values are read, not merely counted**: the sum over the set is the files' sums added
/// up.
#[test]
fn the_aggregates_are_the_files_aggregates_added_up() {
    let (_store, paths) = table_of_commits("aggregate");
    let set = FileSetSource::open(&paths).expect("the engine opens the store's table");

    let plan = sum_plan(KEY_COLUMN);
    let over_set =
        set.execute(&plan, &ExecOptions::default()).expect("the query runs over the set");
    let mut over_files = 0i64;
    for path in &paths {
        let one = open_source(path).expect("the file opens");
        let result = execute_on(&plan, &one, &ExecOptions::default()).expect("the query runs");
        over_files += summed(&result);
    }

    assert_eq!(summed(&over_set), over_files, "the set and the files disagree on the sum");
    let rows = i64::try_from(ROWS * COMMITS).expect("the rows fit");
    assert_eq!(summed(&over_set), rows * (rows - 1) / 2, "the sum is not the sum of the rows");
}
