//! The point lookup: the rows that hold a key.
//!
//! **And the acceptance is the application's**: the lookup answers the rows
//! that hold the key and nothing else; the rows it answers with are the live
//! ones; a table whose schema names no key has no lookup and says so; and the
//! lookup's answer is the answer the plan gives, **so the lookup is a way of
//! asking and not a different question**.
//!
//! **And the key is the schema's own**: a table names one of its columns the
//! key, and the rules — one key, and a key that may not hold a `NULL` — hold
//! wherever a schema comes from, **so a catalog that breaks them is refused
//! rather than read**.

use std::fs;
use std::path::{Path, PathBuf};

use columnar_db::prelude::*;

/// The schema the tests declare: an integer key and a string.
fn schema() -> Schema {
    Schema::new(vec![
        Column::key("id", DataType::Int64),
        Column::new("name", DataType::Utf8, false),
    ])
    .expect("the schema is one a table can hold")
}

/// The rows with those ids, one row each.
fn rows(ids: std::ops::RangeInclusive<i64>) -> Vec<Row> {
    ids.map(|id| Row::filled(vec![ScalarValue::Int64(id), ScalarValue::Utf8(format!("row-{id}"))]))
        .collect()
}

/// The ids of the rows a lookup answers with.
fn ids_of(rows: &Rows) -> Vec<i64> {
    rows.rows()
        .iter()
        .map(|row| match row.get(0) {
            Some(Some(ScalarValue::Int64(id))) => *id,
            other => panic!("the row's id is not an int64: {other:?}"),
        })
        .collect()
}

/// A scratch directory for one test, which removes itself when it is dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A directory under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-db-lookup-{name}-{}", unique()));
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

/// A database with a table of five rows, keyed by `id`.
fn database_with_rows(scratch: &Scratch) -> Database {
    let mut database = Database::open(scratch.path()).expect("the database opens");
    let mut table = database.create_table("items", schema()).expect("the table is created");
    table.insert(&rows(1..=5)).expect("the rows are accepted");
    table.commit().expect("the rows are written");
    database
}

#[test]
fn a_lookup_answers_the_rows_that_hold_the_key() {
    let scratch = Scratch::new("answer");
    let database = database_with_rows(&scratch);

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    let found = table.lookup(3i64).expect("the lookup answers");
    assert_eq!(ids_of(&found), vec![3]);
    let first = found.rows().first().expect("the lookup answered with a row");
    assert_eq!(first.get(1), Some(&Some(ScalarValue::Utf8("row-3".to_owned()))));
}

#[test]
fn a_lookup_that_finds_nothing_answers_nothing() {
    let scratch = Scratch::new("empty");
    let database = database_with_rows(&scratch);

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    let found = table.lookup(99i64).expect("the lookup answers");
    assert!(found.rows().is_empty());
}

#[test]
fn the_lookup_and_the_plan_agree() {
    let scratch = Scratch::new("agree");
    let database = database_with_rows(&scratch);

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    // **The same question, asked two ways.**
    let plan = table.plan().filter(col("id").eq(4i64)).build().expect("the plan is built");
    let asked =
        table.rows(&plan, &columnar_engine::ExecOptions::default()).expect("the rows are read");
    let looked_up = table.lookup(4i64).expect("the lookup answers");

    assert_eq!(ids_of(&asked), ids_of(&looked_up));
}

#[test]
fn the_lookup_answers_the_live_rows() {
    let scratch = Scratch::new("live");
    let mut database = database_with_rows(&scratch);

    // **The row is gone, and the tombstone is what says so.**
    {
        let mut table = database.table_mut("items").expect("the table is there");
        let deleted = table.delete(col("id").eq(2i64)).expect("the delete lands");
        assert_eq!(deleted, 1);
        table.commit().expect("the rows are written");
    }

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    let found = table.lookup(2i64).expect("the lookup answers");
    assert!(found.rows().is_empty(), "a deleted row is not a row");
}

#[test]
fn a_table_without_a_key_has_no_lookup() {
    let scratch = Scratch::new("nokey");
    let mut database = Database::open(scratch.path()).expect("the database opens");
    let schema = Schema::new(vec![
        Column::new("id", DataType::Int64, false),
        Column::new("name", DataType::Utf8, false),
    ])
    .expect("the schema is one a table can hold");
    let mut table = database.create_table("items", schema).expect("the table is created");
    table.insert(&rows(1..=3)).expect("the rows are accepted");
    table.commit().expect("the rows are written");

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    match table.lookup(2i64) {
        Err(columnar_db::Error::NoKey { name }) => assert_eq!(name, "items"),
        other => panic!("the lookup was not refused: {other:?}"),
    }
}

#[test]
fn two_keys_are_refused() {
    let columns = vec![Column::key("id", DataType::Int64), Column::key("other", DataType::Int64)];
    assert!(Schema::new(columns).is_err(), "two keys are not a schema");
}

#[test]
fn a_key_of_the_wrong_type_is_refused() {
    let scratch = Scratch::new("wrongtype");
    let database = database_with_rows(&scratch);

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    match table.lookup("two") {
        Err(columnar_db::Error::ValueType { column, .. }) => assert_eq!(column, "id"),
        other => panic!("the lookup was not refused: {other:?}"),
    }
}

#[test]
fn a_catalog_whose_key_may_hold_a_null_is_refused() {
    let scratch = Scratch::new("badcatalog");
    let database = database_with_rows(&scratch);
    drop(database);

    // **A catalog written by hand, with a key that may hold a NULL.**
    let catalog = r#"{
  "format": "columnar-db/catalog/1",
  "tables": {
    "items": {
      "schema": {
        "columns": [
          { "name": "id", "type": "int64", "nullable": true, "key": true },
          { "name": "name", "type": "utf8", "nullable": false, "key": false }
        ]
      },
      "path": "tables/items"
    }
  }
}"#;
    fs::write(scratch.path().join("catalog.json"), catalog).expect("the catalog is written");

    match Database::open(scratch.path()) {
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains("NULL"), "the refusal names the rule: {message}");
        }
        Ok(_) => panic!("the catalog was read"),
    }
}

#[test]
fn a_catalog_with_two_keys_is_refused() {
    let scratch = Scratch::new("twokeys");
    let database = database_with_rows(&scratch);
    drop(database);

    let catalog = r#"{
  "format": "columnar-db/catalog/1",
  "tables": {
    "items": {
      "schema": {
        "columns": [
          { "name": "id", "type": "int64", "nullable": false, "key": true },
          { "name": "other", "type": "int64", "nullable": false, "key": true }
        ]
      },
      "path": "tables/items"
    }
  }
}"#;
    fs::write(scratch.path().join("catalog.json"), catalog).expect("the catalog is written");

    match Database::open(scratch.path()) {
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains("key"), "the refusal names the rule: {message}");
        }
        Ok(_) => panic!("the catalog was read"),
    }
}

#[test]
fn a_catalog_written_before_the_key_existed_is_read() {
    let scratch = Scratch::new("oldcatalog");
    let database = database_with_rows(&scratch);
    drop(database);

    // **A catalog with no key at all**: the key is optional in the JSON, and a
    // table without a key has no point lookup -- and is still a table.
    let catalog = r#"{
  "format": "columnar-db/catalog/1",
  "tables": {
    "items": {
      "schema": {
        "columns": [
          { "name": "id", "type": "int64", "nullable": false },
          { "name": "name", "type": "utf8", "nullable": false }
        ]
      },
      "path": "tables/items"
    }
  }
}"#;
    fs::write(scratch.path().join("catalog.json"), catalog).expect("the catalog is written");

    let database = Database::open(scratch.path()).expect("the catalog is read");
    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");
    assert!(table.schema().key().is_none(), "the table has no key");
}

#[test]
fn the_lookup_reads_the_files_the_table_has() {
    let scratch = Scratch::new("files");
    let database = database_with_rows(&scratch);

    let snapshot = database.snapshot().expect("the snapshot is taken");
    let table = snapshot.table("items").expect("the table is there");

    // **The lookup answers the application's rows**: the hidden columns are
    // not in the answer, and the columns are named.
    let found = table.lookup(5i64).expect("the lookup answers");
    assert_eq!(found.columns(), ["id", "name"]);
}
