//! The rows that change: the hidden id, the tombstone, the update, the
//! delete, and the compaction.
//!
//! **The acceptance is the application's.** The application never sees the
//! row id, a row that changes leaves the old file alone, a delete leaves a
//! tombstone behind it, and the compaction rewrites the live rows into one
//! file. **And every one of those is checked from the outside**, through the
//! database's own surface and through the files on disk — because a database
//! that promises an append-only model has to be caught if it rewrites
//! anything.

use std::fs;
use std::path::{Path, PathBuf};

use columnar_db::prelude::*;
use columnar_engine::{
    AggregateValue, Dataset, Encoding, ExecOptions, FileSetSource, QueryPlan, ScalarValue,
};

/// The schema the test declares: an integer key and a string.
fn schema() -> Schema {
    Schema::new(vec![
        Column::new("id", DataType::Int64, false),
        Column::new("name", DataType::Utf8, false),
    ])
    .expect("the schema is one a table can hold")
}

/// The rows with those ids, one row each.
fn rows(ids: std::ops::RangeInclusive<i64>) -> Vec<Row> {
    ids.map(|id| Row::filled(vec![ScalarValue::Int64(id), ScalarValue::Utf8(format!("row-{id}"))]))
        .collect()
}

/// A scratch directory for one test, which removes itself when it is dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A directory under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-db-versions-{name}-{}", unique()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("the directory is created");
        Self { path }
    }

    /// The directory's path.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

/// A number that is different every time it is called.
fn unique() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// The files of a table, in the order the manifest names them.
fn files_of(root: &Path, table: &str) -> Vec<PathBuf> {
    let dir = root.join("tables").join(table);
    // **A table with no rows has no directory**: the store makes it when the
    // first file is written.
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("the table's directory reads")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("parquet"))
        .collect();
    files.sort();
    files
}

/// The file at that position, if it is there.
fn file_at(files: &[PathBuf], index: usize) -> PathBuf {
    files.get(index).expect("the file is there").clone()
}

/// The rows of a file, read by the engine.
fn rows_of(path: &Path) -> Dataset {
    let source = FileSetSource::open(&[path.to_path_buf()]).expect("the source opens");
    source.execute_rows(&QueryPlan::scan_all(), &ExecOptions::default()).expect("the rows are read")
}

/// The integers of one column of a dataset.
fn column_of(dataset: &Dataset, name: &str) -> Vec<i64> {
    let chunk = dataset.chunk(name).expect("the file holds the column");
    match &chunk.encoding {
        Encoding::Int64 { values } => values.clone(),
        other => panic!("the column {name} is not an int64: {other:?}"),
    }
}

/// The ids of the rows a read answers with.
fn ids_of(rows: &Rows) -> Vec<i64> {
    rows.rows()
        .iter()
        .map(|row| match row.get(0) {
            Some(Some(ScalarValue::Int64(id))) => *id,
            other => panic!("the row's id is not an int64: {other:?}"),
        })
        .collect()
}

#[test]
fn the_hidden_columns_are_not_the_applications() {
    let scratch = Scratch::new("hidden");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The read holds the application's columns**, and not the database's.
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(read.columns().iter().map(String::as_str).collect::<Vec<_>>(), vec!["id", "name"]);

    // **And a plan that names a hidden column is refused**, rather than
    // answered with the database's own bookkeeping.
    let plan = table.plan().projection(["row_id"]).build().expect("the plan is built");
    let error = table.rows(&plan, &ExecOptions::default()).expect_err("the plan is refused");
    assert!(error.to_string().contains("the column row_id is the database's own"), "{error}");

    // **And a schema that uses a hidden column's name is refused too.**
    let hidden = Schema::new(vec![
        Column::new("id", DataType::Int64, false),
        Column::new("row_id", DataType::Int64, false),
    ])
    .expect("the schema is one a table can hold");
    let error = database.create_table("hidden", hidden).expect_err("the schema is refused");
    assert!(error.to_string().contains("the column row_id is the database's own"), "{error}");
}

#[test]
fn an_update_replaces_the_row() {
    let scratch = Scratch::new("update");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        let mut table = database.table_mut("t").expect("the table is there");
        let changed = table
            .update(
                col("id").eq(2),
                Row::filled(vec![ScalarValue::Int64(99), ScalarValue::Utf8("new".to_owned())]),
            )
            .expect("the update lands");
        assert_eq!(changed, 1);
    }

    // **The row is the new one**, and the table still holds three rows.
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![1, 3, 99]);
    assert_eq!(
        read.get(2).expect("the row is there").values().get(1),
        Some(&Some(ScalarValue::Utf8("new".to_owned())))
    );
}

#[test]
fn an_update_that_matches_nothing_changes_nothing() {
    let scratch = Scratch::new("no-match");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        let mut table = database.table_mut("t").expect("the table is there");
        let changed = table
            .update(
                col("id").eq(404),
                Row::filled(vec![ScalarValue::Int64(0), ScalarValue::Utf8("none".to_owned())]),
            )
            .expect("the update lands");
        assert_eq!(changed, 0);
    }

    // **And no file was written for it**: the table still holds one file.
    assert_eq!(files_of(scratch.path(), "t").len(), 1);
}

#[test]
fn a_delete_removes_the_row() {
    let scratch = Scratch::new("delete");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        let mut table = database.table_mut("t").expect("the table is there");
        let deleted = table.delete(col("id").eq(2)).expect("the delete lands");
        assert_eq!(deleted, 1);
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![1, 3]);
}

#[test]
fn the_old_file_is_never_rewritten() {
    let scratch = Scratch::new("append-only");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    let before = fs::read(file_at(&files_of(scratch.path(), "t"), 0)).expect("the file reads");
    {
        let mut table = database.table_mut("t").expect("the table is there");
        table
            .update(
                col("id").eq(2),
                Row::filled(vec![ScalarValue::Int64(99), ScalarValue::Utf8("new".to_owned())]),
            )
            .expect("the update lands");
    }

    // **The old file is byte for byte the file it was**, because the files are
    // append-only and a version is retired by a tombstone and not by a
    // rewrite.
    let files = files_of(scratch.path(), "t");
    assert_eq!(files.len(), 2, "the update wrote a second file");
    let after = fs::read(file_at(&files, 0)).expect("the file reads");
    assert_eq!(before, after);

    // **And the tombstone is in the new file, naming the version it retires.**
    let written = rows_of(&file_at(&files, 1));
    assert_eq!(column_of(&written, "deleted"), vec![1, 0]);
    assert_eq!(column_of(&written, "row_id"), vec![1, 3]);
}

#[test]
fn a_reader_that_pinned_the_old_file_sees_the_old_row() {
    let scratch = Scratch::new("pinned");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The snapshot is taken here**, with one file in the table.
    let snapshot = database.snapshot().expect("the snapshot is taken");
    {
        let mut table = database.table_mut("t").expect("the table is there");
        table
            .update(
                col("id").eq(2),
                Row::filled(vec![ScalarValue::Int64(99), ScalarValue::Utf8("new".to_owned())]),
            )
            .expect("the update lands");
    }

    // **The pinned reader still sees the old row**, because the file it
    // pinned is unchanged and the tombstone is in a file it never saw.
    let pinned = snapshot.table("t").expect("the table is there");
    let plan = pinned.plan().build().expect("the plan is built");
    let read = pinned.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![1, 2, 3]);

    // **And a new reader sees the new row.**
    let fresh = database.snapshot().expect("the snapshot is taken");
    let table = fresh.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![1, 3, 99]);
}

#[test]
fn the_compaction_rewrites_the_live_rows_into_one_file() {
    let scratch = Scratch::new("compact");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        let mut table = database.table_mut("t").expect("the table is there");
        table
            .update(
                col("id").eq(2),
                Row::filled(vec![ScalarValue::Int64(99), ScalarValue::Utf8("new".to_owned())]),
            )
            .expect("the update lands");
        table.delete(col("id").eq(3)).expect("the delete lands");
    }

    {
        let mut table = database.table_mut("t").expect("the table is there");
        let compaction = table.compact().expect("the table is compacted");
        assert_eq!(compaction.rows, 2);
    }

    // **The table holds one file now**, and it holds the live rows.
    let files = files_of(scratch.path(), "t");
    assert_eq!(files.len(), 1);
    let written = rows_of(&file_at(&files, 0));
    assert_eq!(written.len(), 2);
    assert_eq!(column_of(&written, "deleted"), vec![0, 0]);

    // **And the read answers with the same rows the read answered with
    // before the compaction.**
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![1, 99]);
}

#[test]
fn a_compaction_crosses_the_grid() {
    // **A table of more than a grid's worth of rows**, so the compaction's
    // output holds a grid's worth of rows in one batch and the rest in
    // another -- and the rows come from more than one file.
    let scratch = Scratch::new("grid");
    let grid = columnar_store::BATCH_ROWS;

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        let ids = 1..=i64::try_from(grid + 1_000).expect("the rows fit");
        table.insert(&rows(ids)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
        // **And a row is retired**, so the compaction has something to leave
        // behind.
        table.delete(col("id").eq(1)).expect("the delete lands");
    }

    // **The table holds more than one file**, so the compaction reads more
    // than one.
    let before = files_of(scratch.path(), "t");
    assert!(before.len() > 1, "the table holds more than one file");

    {
        let mut table = database.table_mut("t").expect("the table is there");
        let compaction = table.compact().expect("the table is compacted");
        assert_eq!(
            compaction.rows as usize,
            grid + 1_000 - 1,
            "the live rows are the ones that are left"
        );
    }

    // **And the table holds one file now**, and it holds every live row.
    let files = files_of(scratch.path(), "t");
    assert_eq!(files.len(), 1, "the compaction leaves one file");
    let written = rows_of(&file_at(&files, 0));
    assert_eq!(written.len(), grid + 1_000 - 1, "the file holds every live row");

    // **And the versions are numbered again**, across the files the rows came
    // from: 0, 1, 2, ... in the order they are written.
    let ids = column_of(&written, "row_id");
    assert_eq!(ids.len(), grid + 1_000 - 1);
    assert_eq!(ids.first().copied(), Some(0), "the first version is 0");
    let last = i64::try_from(grid + 1_000 - 2).expect("the rows fit");
    assert_eq!(ids.last().copied(), Some(last), "and the last is the row before it");
    assert!(ids.windows(2).all(|pair| pair[1] == pair[0] + 1), "the versions are consecutive");
}

#[test]
fn a_compacted_table_numbers_its_versions_again() {
    let scratch = Scratch::new("renumber");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
        table
            .update(
                col("id").eq(2),
                Row::filled(vec![ScalarValue::Int64(99), ScalarValue::Utf8("new".to_owned())]),
            )
            .expect("the update lands");
        table.compact().expect("the table is compacted");
    }

    // **The compacted table's versions are 0, 1, ... in the order they were
    // written**, and the next version gets the number of rows the table has.
    // The three live rows are the ones the update did not retire.
    let files = files_of(scratch.path(), "t");
    let written = rows_of(&file_at(&files, 0));
    assert_eq!(column_of(&written, "row_id"), vec![0, 1, 2]);

    {
        let mut table = database.table_mut("t").expect("the table is there");
        table
            .update(
                col("id").eq(1),
                Row::filled(vec![ScalarValue::Int64(7), ScalarValue::Utf8("seven".to_owned())]),
            )
            .expect("the update lands");
    }

    // **And the new version does not collide with a version that is already
    // there**: the tombstone names the version it retires, and the new
    // version gets the number of versions the table has -- which is the three
    // the compaction left.
    let files = files_of(scratch.path(), "t");
    let written = rows_of(&file_at(&files, 1));
    assert_eq!(column_of(&written, "row_id"), vec![0, 3]);
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![3, 99, 7]);
}

#[test]
fn the_old_files_stay_while_a_reader_holds_them() {
    let scratch = Scratch::new("reclaim");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The reader is here**, and it holds the file the compaction wants to
    // remove.
    let snapshot = database.snapshot().expect("the snapshot is taken");
    {
        let mut table = database.table_mut("t").expect("the table is there");
        let compaction = table.compact().expect("the table is compacted");
        assert_eq!(compaction.rows, 3);
        assert_eq!(compaction.pending, 1, "the reader holds the old file");
    }
    let files = files_of(scratch.path(), "t");
    assert_eq!(files.len(), 2, "the old file is still on disk");

    // **And the reader is gone**, so the next compaction reclaims it.
    drop(snapshot);
    {
        let mut table = database.table_mut("t").expect("the table is there");
        let compaction = table.compact().expect("the table is compacted");
        assert_eq!(compaction.pending, 0);
        assert!(compaction.reclaimed >= 1);
    }
    assert_eq!(files_of(scratch.path(), "t").len(), 1, "the old files are gone");
}

#[test]
fn the_next_write_does_not_clobber_the_old_file() {
    let scratch = Scratch::new("naming");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The reader is here**, so the compaction leaves the old file where it
    // is -- and the next write has to step over it.
    let _snapshot = database.snapshot().expect("the snapshot is taken");
    {
        let mut table = database.table_mut("t").expect("the table is there");
        table.compact().expect("the table is compacted");
        table.insert(&rows(4..=4)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **Three files, three names**: the compaction did not reset the
    // numbering and land the next write on the file the compaction wrote.
    let files = files_of(scratch.path(), "t");
    assert_eq!(files.len(), 3);
    let names: Vec<String> = files
        .iter()
        .map(|path| path.file_name().expect("the file has a name").to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["000001.parquet", "000002.parquet", "000003.parquet"]);

    // **And the table still reads.**
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(ids_of(&read), vec![1, 2, 3, 4]);
}

#[test]
fn a_table_with_no_rows_compacts_to_nothing() {
    let scratch = Scratch::new("empty-compact");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        let compaction = table.compact().expect("the table is compacted");
        assert_eq!(compaction.rows, 0);
    }

    // **And no file was written for it**, because a file with no rows is not
    // a file.
    assert!(files_of(scratch.path(), "t").is_empty());
}

#[test]
fn an_aggregate_counts_the_live_rows() {
    let scratch = Scratch::new("aggregate");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
        table.delete(col("id").eq(2)).expect("the delete lands");
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table
        .plan()
        .aggregate(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] })
        .build()
        .expect("the plan is built");
    let result = table.execute(&plan, &ExecOptions::default()).expect("the aggregate runs");

    // **The tombstone is not a row**, so the count is the live rows.
    assert_eq!(result.groups.len(), 1);
    let group = result.groups.first().expect("the aggregate has a group");
    assert_eq!(group.values, vec![AggregateValue::Int(2)]);
}

#[test]
fn a_join_build_side_holds_the_live_rows() {
    let scratch = Scratch::new("join-build");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("probe", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        let mut table = database.create_table("build", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
        table.delete(col("id").eq(2)).expect("the delete lands");
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let probe = snapshot.table("probe").expect("the table is there");
    let plan = probe
        .plan()
        .join(TableAddress::Named("build"), "id", "id")
        .build()
        .expect("the plan is built");
    let read = probe.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");

    // **The deleted row of the build side is not in the hash table**, so the
    // probe row that would have matched it does not match.
    assert_eq!(read.len(), 2);
}
