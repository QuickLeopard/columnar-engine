//! The catalog survives a restart, and it is replaced by a rename.
//!
//! **The acceptance is the one a database has to meet to be a database at
//! all**: the names are still there after the process that wrote them is
//! gone. Everything else in this crate is the engine's and the store's, and
//! this is the one thing that is the database's own.

use std::fs;
use std::path::{Path, PathBuf};

use columnar_db::prelude::*;
use columnar_db::{CATALOG_FILE, CATALOG_TMP};

/// A scratch directory for one test, which removes itself when it is dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A directory under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-db-{name}-{}", unique()));
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

/// The schema the tests declare: an integer key and a nullable string.
fn schema() -> Schema {
    Schema::new(vec![
        Column::new("id", DataType::Int64, false),
        Column::new("name", DataType::Utf8, true),
    ])
    .expect("the schema is one a table can hold")
}

#[test]
fn the_catalog_survives_a_reopen() {
    let scratch = Scratch::new("reopen");

    {
        let mut database = Database::open(scratch.path()).expect("the database opens");
        database.create_table("lineitem", schema()).expect("the table is created");
        database.create_table("part", schema()).expect("the second table is created");
    }

    let database = Database::open(scratch.path()).expect("the database opens again");
    assert_eq!(database.tables(), vec!["lineitem".to_owned(), "part".to_owned()]);

    let table = database.table("lineitem").expect("the table is still there");
    assert_eq!(table.name(), "lineitem");
    assert_eq!(table.schema().len(), 2);
    assert_eq!(table.schema().names().collect::<Vec<_>>(), vec!["id", "name"]);
    assert_eq!(table.schema().column("id").map(Column::data_type), Some(DataType::Int64));
    assert!(table.schema().column("id").is_some_and(|column| !column.is_nullable()));
    assert!(table.schema().column("name").is_some_and(Column::is_nullable));
}

#[test]
fn a_table_with_no_rows_is_a_table() {
    let scratch = Scratch::new("empty");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    let table = database.create_table("lineitem", schema()).expect("the table is created");

    // **The table exists as soon as the catalog says so.** Its directory is
    // made when the first rows are written, because the store makes it then.
    assert!(!table.path().exists());
    assert_eq!(table.path(), scratch.path().join("tables").join("lineitem"));

    let database = Database::open(scratch.path()).expect("the database opens again");
    assert!(database.table("lineitem").is_some());
}

#[test]
fn the_catalog_is_replaced_by_a_rename() {
    let scratch = Scratch::new("rename");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    database.create_table("lineitem", schema()).expect("the table is created");

    // The rename consumed the temporary file, so there is no half-written
    // catalog left behind.
    assert!(scratch.path().join(CATALOG_FILE).is_file());
    assert!(!scratch.path().join(CATALOG_TMP).exists());
}

#[test]
fn the_catalog_is_json() {
    let scratch = Scratch::new("json");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    database.create_table("lineitem", schema()).expect("the table is created");

    let text = fs::read_to_string(scratch.path().join(CATALOG_FILE)).expect("the catalog reads");
    assert!(text.contains("columnar-db/catalog/1"), "the format is written: {text}");
    assert!(text.contains("lineitem"), "the table is named: {text}");
    assert!(text.contains("int64"), "the type is the engine's own name: {text}");
}

#[test]
fn a_table_that_is_created_twice_is_refused() {
    let scratch = Scratch::new("twice");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    database.create_table("lineitem", schema()).expect("the table is created");

    let error = database
        .create_table("lineitem", schema())
        .expect_err("the second table with that name is refused");
    assert!(error.to_string().contains("already holds a table named lineitem"), "{error}");
}

#[test]
fn a_name_that_is_not_a_name_is_refused() {
    let scratch = Scratch::new("names");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    for name in ["", ".", "..", "a/b", "a\\b"] {
        let error = database.create_table(name, schema()).expect_err("the name is refused");
        assert!(error.to_string().contains("not one a database can hold"), "{error}");
    }
}

#[test]
fn a_catalog_that_cannot_be_read_is_refused() {
    let scratch = Scratch::new("corrupt");

    fs::write(scratch.path().join(CATALOG_FILE), "{ this is not a catalog")
        .expect("the file is written");

    let error = Database::open(scratch.path()).expect_err("the catalog is refused");
    assert!(error.to_string().contains("not one this database can read"), "{error}");
}

#[test]
fn a_directory_with_no_catalog_is_an_empty_database() {
    let scratch = Scratch::new("empty-dir");
    let root = scratch.path().join("not-there-yet");

    let database = Database::open(&root).expect("the database opens");
    assert!(database.tables().is_empty());
    assert!(root.is_dir(), "the directory is created");
    assert!(!root.join(CATALOG_FILE).exists(), "and no catalog is written until it is needed");
}

#[test]
fn a_schema_with_two_columns_of_the_same_name_is_refused() {
    let error = Schema::new(vec![
        Column::new("id", DataType::Int64, false),
        Column::new("id", DataType::Utf8, false),
    ])
    .expect_err("the schema is refused");
    assert!(error.to_string().contains("two columns are named id"), "{error}");
}
