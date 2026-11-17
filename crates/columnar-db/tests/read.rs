//! The snapshot pins the manifest, and the read path is the engine's plan.
//!
//! **The acceptance is a reader's**: it sees one version of a table, and the
//! version it sees does not move while it reads. **And the query language is
//! the engine's** — the plan is built with the engine's own builder, over the
//! engine's own expressions, and the engine's surface is not widened to serve
//! the database.

use std::fs;
use std::path::{Path, PathBuf};

use columnar_db::prelude::*;

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
        let path = std::env::temp_dir().join(format!("columnar-db-read-{name}-{}", unique()));
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

#[test]
fn a_reader_sees_the_old_file_set_or_the_new_one() {
    let scratch = Scratch::new("old-or-new");

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
        table.insert(&rows(4..=6)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The snapshot still sees one file**, and the write that landed after it
    // is not in it.
    let pinned = snapshot.table("t").expect("the table is there");
    assert_eq!(pinned.files().len(), 1);
    let plan = pinned.plan().build().expect("the plan is built");
    let read = pinned.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(read.len(), 3);

    // **And a new snapshot sees both.**
    let fresh = database.snapshot().expect("the snapshot is taken");
    let table = fresh.table("t").expect("the table is there");
    assert_eq!(table.files().len(), 2);
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(read.len(), 6);
}

#[test]
fn the_rows_come_back() {
    let scratch = Scratch::new("rows");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");

    assert_eq!(read.len(), 3);
    assert_eq!(read.columns().iter().map(String::as_str).collect::<Vec<_>>(), vec!["id", "name"]);
    // **The rows are the engine's values**, in the database's row shape.
    assert_eq!(
        read.get(0).expect("the row is there").values().first(),
        Some(&Some(ScalarValue::Int64(1)))
    );
    assert_eq!(
        read.get(2).expect("the row is there").values().get(1),
        Some(&Some(ScalarValue::Utf8("row-3".to_owned())))
    );
}

#[test]
fn a_filter_reaches_every_file() {
    let scratch = Scratch::new("filter");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        let mut table = database.table_mut("t").expect("the table is there");
        table.insert(&rows(4..=6)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The predicate reaches every file**, because the table is read as one
    // table -- the engine's own property, reached through the database.
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().filter(col("id").gt(2)).build().expect("the plan is built");
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert_eq!(read.len(), 4);
}

#[test]
fn the_join_build_side_is_the_whole_table() {
    let scratch = Scratch::new("join");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("probe", schema()).expect("the table is created");
        table.insert(&rows(1..=6)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }
    {
        // **The build side is two files**, so a build side that was only the
        // first file would answer three rows and the whole table answers six.
        let mut table = database.create_table("build", schema()).expect("the table is created");
        table.insert(&rows(1..=3)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
        table.insert(&rows(4..=6)).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let probe = snapshot.table("probe").expect("the table is there");
    let plan = probe
        .plan()
        .join(TableAddress::Named("build"), "id", "id")
        .build()
        .expect("the plan is built");
    let read = probe.execute_rows(&plan, &ExecOptions::default()).expect("the rows are read");

    assert_eq!(read.len(), 6);
}

#[test]
fn a_table_with_no_rows_reads_as_no_rows() {
    let scratch = Scratch::new("empty");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    database.create_table("t", schema()).expect("the table is created");

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("t").expect("the table is there");
    let plan = table.plan().build().expect("the plan is built");

    // **A table with no rows reads as no rows**, and it still says which
    // columns it has.
    let read = table.rows(&plan, &ExecOptions::default()).expect("the rows are read");
    assert!(read.is_empty());
    assert_eq!(read.columns().iter().map(String::as_str).collect::<Vec<_>>(), vec!["id", "name"]);

    // **And an aggregate over a table with no rows is refused**, rather than
    // answered with a zero the engine never computed.
    let error =
        table.execute(&plan, &ExecOptions::default()).expect_err("the aggregate is refused");
    assert!(error.to_string().contains("has no rows"), "{error}");
}

#[test]
fn a_snapshot_names_the_tables_the_database_holds() {
    let scratch = Scratch::new("names");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    database.create_table("lineitem", schema()).expect("the table is created");
    database.create_table("part", schema()).expect("the second table is created");

    let snapshot = database.snapshot().expect("the snapshot is taken");
    assert_eq!(snapshot.tables(), vec!["lineitem".to_owned(), "part".to_owned()]);
    assert!(snapshot.table("lineitem").is_some());
    assert!(snapshot.table("nope").is_none());
}
