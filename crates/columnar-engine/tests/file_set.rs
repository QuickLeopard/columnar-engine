//! A set of files is read as **one table**: the same rows, in the same order,
//! as the same rows in one file.
//!
//! # Why this file exists
//!
//! The store writes a table as a **set of files** -- a commit appends one and
//! never modifies an old one -- and the manifest names them. **And the
//! engine's only source is one file**, so the embedder's way of reading a
//! store's table today is to open each file and run the query over each one,
//! and that is **wrong three times over**: the rows of a filtered table are
//! split across the files, the groups of an aggregate are split across them,
//! and the sides of a join are split across them.
//!
//! So the set is read as one table, and this file is what holds the new source
//! to it. Every test here answers the same question twice -- once over the
//! one file, once over the set -- and the two answers have to be identical.
//!
//! # What is checked here
//!
//! - **The rows.** The set's rows are the one file's rows, row by row, in
//!   order, and the set's rows keep the **files' order**: file by file, and
//!   inside a file row group by row group.
//! - **The predicate.** The engine's own predicate reaches **every** file, so
//!   a filtered set holds the rows the one file holds.
//! - **The statistics.** The set's range for a column is the **union** of the
//!   files' ranges, and the row groups are numbered across the files.
//! - **The schemas.** Files that disagree are refused, and the error says
//!   which file disagreed and how.
//! - **The parallel read.** The same rows, in the same order, however many
//!   threads read them.

// The chunk indices below are literals into a dataset whose length the test
// has just asserted, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_core::{ComparisonOp, Predicate, ScalarValue};
use columnar_engine::pushdown::PushdownPlan;
use columnar_engine::{
    execute_on, Aggregate, AggregateSpec, AggregateValue, ExecOptions, FileSetSource,
    ParquetSource, QueryPlan,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

/// The rows in each row group of the written file.
const ROW_GROUP_ROWS: usize = 64;

/// The rows in the written file.
const ROWS: usize = 256;

/// The column the rows are compared on.
const KEY_COLUMN: &str = "k";

/// The column the names are compared on.
const NAME_COLUMN: &str = "v";

/// A temporary file that removes itself when it is dropped.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A path under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-file-set-{name}-{}", unique()));
        Self { path }
    }

    /// The file's path.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// A number that is different every time it is called.
fn unique() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// A temporary directory that removes itself when it is dropped.
struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    /// A directory under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-file-set-{name}-{}", unique()));
        std::fs::create_dir_all(&path).expect("the directory is created");
        Self { path }
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// The schema the tests write: an integer key and a string.
fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(KEY_COLUMN, DataType::Int64, false),
        Field::new(NAME_COLUMN, DataType::Utf8, false),
    ]))
}

/// Writes `count` rows, with the keys counting from `first`.
///
/// The names repeat, so the names exercise the **dictionary** the reader
/// builds and the merge has to combine.
fn write(path: &Path, first: usize, count: usize, row_group_rows: usize) {
    let schema = schema();
    let file = File::create(path).expect("the file is created");
    let properties =
        WriterProperties::builder().set_max_row_group_row_count(Some(row_group_rows)).build();
    let mut writer =
        ArrowWriter::try_new(file, schema.clone(), Some(properties)).expect("the writer is built");
    for start in (0..count).step_by(row_group_rows) {
        let take = row_group_rows.min(count - start);
        let keys: Vec<i64> = (start..start + take)
            .map(|offset| i64::try_from(first + offset).unwrap_or(0))
            .collect();
        let names: Vec<String> =
            (start..start + take).map(|offset| format!("name-{}", (first + offset) % 7)).collect();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(keys)), Arc::new(StringArray::from(names))],
        )
        .expect("the batch matches the schema");
        writer.write(&batch).expect("the batch is written");
    }
    writer.close().expect("the file is finished");
}

/// Writes the rows as one file per row group, and returns the files.
fn write_split(directory: &Path, rows: usize, row_group_rows: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut index = 0usize;
    let mut start = 0usize;
    while start < rows {
        let path = directory.join(format!("{index:06}.parquet"));
        write(&path, start, row_group_rows, row_group_rows);
        files.push(path);
        index += 1;
        start += row_group_rows;
    }
    files
}

/// Reads the given columns of a source, through the engine's own reader.
fn read(source: &ParquetSource, columns: &[usize]) -> columnar_engine::Dataset {
    source.read(columns, &PushdownPlan::new(), columnar_kernels::best(), 1).expect("the read runs")
}

/// Reads the given columns of a set, through the engine's own reader.
fn read_set(source: &FileSetSource, columns: &[usize]) -> columnar_engine::Dataset {
    source.read(columns, &PushdownPlan::new(), columnar_kernels::best(), 1).expect("the read runs")
}

/// Whether two values are the same value.
fn same_value(left: Option<ScalarValue>, right: Option<ScalarValue>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(ScalarValue::Int64(a)), Some(ScalarValue::Int64(b))) => a == b,
        (Some(ScalarValue::Float64(a)), Some(ScalarValue::Float64(b))) => {
            a.to_bits() == b.to_bits()
        }
        (Some(ScalarValue::Utf8(a)), Some(ScalarValue::Utf8(b))) => a == b,
        _ => false,
    }
}

/// Asserts that two datasets hold the same rows, row by row, in order.
fn assert_same_rows(left: &columnar_engine::Dataset, right: &columnar_engine::Dataset) {
    assert_eq!(left.len(), right.len(), "the two datasets hold different row counts");
    for column in [KEY_COLUMN, NAME_COLUMN] {
        let left_column = left.chunk(column).expect("the column is there");
        let right_column = right.chunk(column).expect("the column is there");
        for row in 0..left.len() {
            assert!(
                same_value(left_column.value_at(row), right_column.value_at(row)),
                "row {row} of column `{column}` differs: {:?} against {:?}",
                left_column.value_at(row),
                right_column.value_at(row)
            );
        }
    }
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

/// A plan that counts the rows a predicate keeps.
fn count_plan(column: &str, op: ComparisonOp, value: ScalarValue) -> QueryPlan {
    QueryPlan::new(
        vec![Predicate::new(column, op, value)],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// The set holds the one file's rows, row by row, in order.
#[test]
fn the_set_holds_the_one_files_rows() {
    let one = TempFile::new("one");
    write(one.path(), 0, ROWS, ROW_GROUP_ROWS);
    let directory = TempDirectory::new("split");
    let files = write_split(&directory.path, ROWS, ROW_GROUP_ROWS);
    assert_eq!(files.len(), ROWS / ROW_GROUP_ROWS);

    let source = ParquetSource::open(one.path()).expect("the source opens");
    let set = FileSetSource::open(&files).expect("the set opens");

    assert_eq!(set.rows(), source.rows(), "the set and the file hold different row counts");
    assert_eq!(
        set.row_groups(),
        source.row_groups(),
        "the set and the file hold different row groups"
    );

    let columns = [0usize, 1];
    let from_file = read(&source, &columns);
    let from_set = read_set(&set, &columns);
    assert_same_rows(&from_file, &from_set);
}

/// The set's rows keep the **files' order**: file by file, and inside a file
/// row group by row group.
///
/// The files are handed over **out of key order**, so a set that sorted its
/// rows would fail this test.
#[test]
fn the_rows_keep_the_files_order() {
    let directory = TempDirectory::new("order");

    let mut paths = Vec::new();
    for index in 0..4usize {
        let path = directory.path.join(format!("{index:06}.parquet"));
        write(&path, index * ROW_GROUP_ROWS, ROW_GROUP_ROWS, ROW_GROUP_ROWS);
        paths.push(path);
    }
    // The files are handed over shuffled: the second, the first, the fourth,
    // the third.
    let files = vec![paths[1].clone(), paths[0].clone(), paths[3].clone(), paths[2].clone()];

    let set = FileSetSource::open(&files).expect("the set opens");
    let dataset = read_set(&set, &[0]);
    let column = dataset.chunk(KEY_COLUMN).expect("the column is there");
    let keys: Vec<i64> = (0..dataset.len())
        .map(|row| match column.value_at(row) {
            Some(ScalarValue::Int64(value)) => value,
            other => panic!("the key is not an integer: {other:?}"),
        })
        .collect();

    let expected: Vec<i64> = [1usize, 0, 3, 2]
        .iter()
        .flat_map(|&file| {
            (0..ROW_GROUP_ROWS)
                .map(move |row| i64::try_from(file * ROW_GROUP_ROWS + row).unwrap_or(0))
        })
        .collect();
    assert_eq!(keys, expected, "the rows are not in the files' order");
}

/// The engine's own predicate reaches **every** file.
#[test]
fn the_predicate_reaches_every_file() {
    let one = TempFile::new("predicate");
    write(one.path(), 0, ROWS, ROW_GROUP_ROWS);
    let directory = TempDirectory::new("predicate-split");
    let files = write_split(&directory.path, ROWS, ROW_GROUP_ROWS);

    let source = ParquetSource::open(one.path()).expect("the source opens");
    let set = FileSetSource::open(&files).expect("the set opens");

    for (op, value) in [
        (ComparisonOp::Gt, ScalarValue::Int64(100)),
        (ComparisonOp::Lt, ScalarValue::Int64(100)),
        (ComparisonOp::Eq, ScalarValue::Int64(100)),
        (ComparisonOp::Ne, ScalarValue::Int64(100)),
    ] {
        let plan = count_plan(KEY_COLUMN, op, value.clone());
        let from_file = execute_on(&plan, &source, &ExecOptions::default())
            .expect("the query runs over the file");
        let from_set =
            set.execute(&plan, &ExecOptions::default()).expect("the query runs over the set");
        assert_eq!(
            counted(&from_file),
            counted(&from_set),
            "the set and the file disagreed on the count for {op:?} {value:?}"
        );
    }
}

/// The set's statistics are the **table's**: the range is the union of the
/// files' ranges, and the row groups are numbered across the files.
#[test]
fn the_statistics_are_merged() {
    let directory = TempDirectory::new("statistics");

    let first = directory.path.join("first.parquet");
    let second = directory.path.join("second.parquet");
    write(&first, 0, ROW_GROUP_ROWS, ROW_GROUP_ROWS);
    write(&second, 1_000, ROW_GROUP_ROWS, ROW_GROUP_ROWS);

    let one = ParquetSource::open(&first).expect("the source opens");
    let other = ParquetSource::open(&second).expect("the source opens");
    let set = FileSetSource::open(&[first, second]).expect("the set opens");

    // The files hold **different** rows, so the union is wider than either
    // file's own range.
    let (low, high) = set.column_range(KEY_COLUMN).expect("the key column has statistics");
    let (ScalarValue::Int64(low), ScalarValue::Int64(high)) = (low, high) else {
        panic!("the key column's range is not an integer");
    };
    let (ScalarValue::Int64(one_low), ScalarValue::Int64(one_high)) =
        one.column_range(KEY_COLUMN).expect("the range")
    else {
        panic!("the key column's range is not an integer");
    };
    let (ScalarValue::Int64(other_low), ScalarValue::Int64(other_high)) =
        other.column_range(KEY_COLUMN).expect("the range")
    else {
        panic!("the key column's range is not an integer");
    };
    assert_eq!(low, one_low.min(other_low), "the set's range does not start at the lowest value");
    assert_eq!(high, one_high.max(other_high), "the set's range does not end at the highest value");

    // The row groups are the table's: the files' row groups, numbered across
    // the files.
    assert_eq!(set.row_groups(), one.row_groups() + other.row_groups());
    let groups = set.row_group_ranges(KEY_COLUMN);
    assert_eq!(groups.len(), set.row_groups(), "the row groups are not the table's");
    assert_eq!(
        groups.iter().map(|group| group.rows).sum::<usize>(),
        set.rows(),
        "the row groups do not hold the table's rows"
    );
}

/// Files that do not share a schema are refused, and the error says which
/// file disagreed and how.
#[test]
fn the_files_must_share_their_schema() {
    let directory = TempDirectory::new("schema");

    let first = directory.path.join("first.parquet");
    let second = directory.path.join("second.parquet");
    write(&first, 0, ROW_GROUP_ROWS, ROW_GROUP_ROWS);

    // The second file holds the **same number of columns**, and a different
    // name for the first of them.
    let other_schema = Arc::new(Schema::new(vec![
        Field::new("other", DataType::Int64, false),
        Field::new(NAME_COLUMN, DataType::Utf8, false),
    ]));
    let file = File::create(&second).expect("the file is created");
    let mut writer = ArrowWriter::try_new(file, other_schema.clone(), None).expect("the writer");
    let batch = RecordBatch::try_new(
        other_schema,
        vec![
            Arc::new(Int64Array::from(vec![1_i64, 2, 3])),
            Arc::new(StringArray::from(vec!["a", "b", "c"])),
        ],
    )
    .expect("the batch matches the schema");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the file is finished");

    let error = FileSetSource::open(&[first, second]).expect_err("the set is refused");
    let message = error.to_string();
    assert!(message.contains("do not share a schema"), "the error does not say why: {message}");
    assert!(message.contains("index 1"), "the error does not name the file: {message}");
    assert!(message.contains("other"), "the error does not name the column: {message}");
}

/// The parallel read gives the same rows, in the same order, as the serial
/// one -- and the same rows as the one file.
#[test]
fn the_parallel_read_gives_the_same_rows() {
    let one = TempFile::new("parallel");
    write(one.path(), 0, ROWS, ROW_GROUP_ROWS);
    let directory = TempDirectory::new("parallel-split");
    let files = write_split(&directory.path, ROWS, ROW_GROUP_ROWS);

    let source = ParquetSource::open(one.path()).expect("the source opens");
    let set = FileSetSource::open(&files).expect("the set opens");

    let from_file = read(&source, &[0, 1]);
    let serial = read_set(&set, &[0, 1]);
    assert_same_rows(&from_file, &serial);

    for threads in [2usize, 4, 6] {
        let parallel = set
            .read(&[0, 1], &PushdownPlan::new(), columnar_kernels::best(), threads)
            .expect("the parallel read runs");
        assert_same_rows(&from_file, &parallel);
    }
}

/// The reader's batch size is derived from the **merged** row groups.
#[test]
fn the_batch_size_is_derived_from_the_merged_row_groups() {
    let one = TempFile::new("batch");
    write(one.path(), 0, ROWS, ROW_GROUP_ROWS);
    let directory = TempDirectory::new("batch-split");
    let files = write_split(&directory.path, ROWS, ROW_GROUP_ROWS);

    let set = FileSetSource::open(&files).expect("the set opens");
    let batch = set.batch_rows_for(&[0], 1);

    // The batch divides every row group's row count, in every file -- so no
    // row group anywhere ends with a partial batch.
    for group in set.row_group_ranges(KEY_COLUMN) {
        assert_eq!(
            group.rows % batch,
            0,
            "a row group of {} rows ends with a partial batch",
            group.rows
        );
    }
}
