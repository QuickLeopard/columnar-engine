//! The database writes the file the store writes, byte for byte.
//!
//! **The acceptance is the store's own, applied to the database.** The rows go
//! to the store twice — once as the engine's chunks, straight to the store,
//! and once as the database's typed values, through the database — and the
//! two files are compared **byte for byte**. So the database's claim is not
//! "it wrote something" but "**it wrote the file the store writes**".
//!
//! **And the reference holds the two hidden columns**, because every file the
//! database writes holds them: the row's own id, and the flag that says the
//! row is not a tombstone. **The bytes are the store's writer's bytes** — the
//! database only decides the rows and the name, and the name is the store's
//! own for the first file of a table.
//!
//! **And the reference's NULL placeholder is different on purpose.** The
//! reference says `-1` and `<null>` where the database says `0` and the empty
//! string. **So if the placeholder ever reached the file, the two files would
//! differ** — and the test would say so, instead of passing on a rule nobody
//! checked.

use std::fs;
use std::path::{Path, PathBuf};

use columnar_db::prelude::*;
use columnar_engine::{
    ColumnChunk, Dataset, Encoding, ExecOptions, FileSetSource, NullBitmap, QueryPlan, ScalarValue,
};
use columnar_store::writer_properties;

/// The rows the test writes: three rows, two columns, one NULL.
fn rows() -> Vec<Row> {
    vec![
        Row::filled(vec![ScalarValue::Int64(1), ScalarValue::Utf8("one".to_owned())]),
        Row::new(vec![Some(ScalarValue::Int64(2)), None]),
        Row::filled(vec![ScalarValue::Int64(3), ScalarValue::Utf8("three".to_owned())]),
    ]
}

/// The schema the test declares: an integer key and a nullable string.
fn schema() -> Schema {
    Schema::new(vec![
        Column::new("id", DataType::Int64, false),
        Column::new("name", DataType::Utf8, true),
    ])
    .expect("the schema is one a table can hold")
}

/// The same rows, as the engine's chunks, built by hand.
///
/// **The placeholder for the NULL is different on purpose**: the reference
/// says `-1` and `<null>` where the database says `0` and the empty string.
///
/// **And the two hidden columns are here too**, because they are in every
/// file the database writes: the row's own id, and the flag that says the
/// row is not a tombstone.
fn reference_chunks() -> Vec<ColumnChunk> {
    vec![
        ColumnChunk::new("id", Encoding::Int64 { values: vec![1, 2, 3] }, NullBitmap::all_valid(3)),
        ColumnChunk::new(
            "name",
            Encoding::Utf8 {
                values: vec!["one".to_owned(), "<null>".to_owned(), "three".to_owned()],
            },
            NullBitmap::with_nulls(3, &[1]),
        ),
        ColumnChunk::new(
            "row_id",
            Encoding::Int64 { values: vec![0, 1, 2] },
            NullBitmap::all_valid(3),
        ),
        ColumnChunk::new(
            "deleted",
            Encoding::Int64 { values: vec![0, 0, 0] },
            NullBitmap::all_valid(3),
        ),
    ]
}

/// The optionality the database writes its files with.
///
/// **The application's columns are optional and the hidden ones are required, and the tombstone is
/// the reason**: a tombstone holds no value of the application's, so the application's columns carry
/// an absent value in every file that holds one — and the `id` and `name` are optional in the file
/// whether this batch holds a NULL in them or not.
fn optionality() -> Vec<bool> {
    vec![true, true, false, false]
}

/// The rows of a table whose columns hold no NULL.
///
/// **The rows are the same rows and not the same rows**: the `name` holds a
/// value in every row, so a file written under the rows' own optionality would
/// say the column may not hold a NULL — and the database's own rule says the
/// application's columns are optional, because a tombstone holds no value in
/// them.
fn rows_without_nulls() -> Vec<Row> {
    vec![
        Row::filled(vec![ScalarValue::Int64(1), ScalarValue::Utf8("one".to_owned())]),
        Row::filled(vec![ScalarValue::Int64(2), ScalarValue::Utf8("two".to_owned())]),
        Row::filled(vec![ScalarValue::Int64(3), ScalarValue::Utf8("three".to_owned())]),
    ]
}

/// The same rows, as the engine's chunks, with no NULL in the `name`.
fn chunks_without_nulls() -> Vec<ColumnChunk> {
    vec![
        ColumnChunk::new("id", Encoding::Int64 { values: vec![1, 2, 3] }, NullBitmap::all_valid(3)),
        ColumnChunk::new(
            "name",
            Encoding::Utf8 { values: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()] },
            NullBitmap::all_valid(3),
        ),
        ColumnChunk::new(
            "row_id",
            Encoding::Int64 { values: vec![0, 1, 2] },
            NullBitmap::all_valid(3),
        ),
        ColumnChunk::new(
            "deleted",
            Encoding::Int64 { values: vec![0, 0, 0] },
            NullBitmap::all_valid(3),
        ),
    ]
}

/// A scratch directory for one test, which removes itself when it is dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A directory under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-db-write-{name}-{}", unique()));
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

/// The one file a table holds, under a database's root.
fn the_file(root: &Path) -> PathBuf {
    root.join("tables").join("t").join("000001.parquet")
}

/// The rows of a file, read by the engine.
fn rows_of(path: &Path) -> Dataset {
    let source = FileSetSource::open(&[path.to_path_buf()]).expect("the source opens");
    source.execute_rows(&QueryPlan::scan_all(), &ExecOptions::default()).expect("the rows are read")
}

#[test]
fn the_database_writes_the_file_the_store_writes() {
    let reference = Scratch::new("reference");
    let written = Scratch::new("written");

    // The reference: the rows, straight to the store.
    {
        let store =
            columnar_store::Store::open(reference.path().join("tables")).expect("the store opens");
        store
            .append_rows("t", &reference_chunks(), &optionality(), writer_properties(&["row_id"]))
            .expect("the rows are written");
    }

    // The database: the same rows, as typed values.
    {
        let mut database = Database::open(written.path()).expect("the database opens");
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows()).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The two files, byte for byte.**
    let reference_bytes = fs::read(the_file(reference.path())).expect("the reference file reads");
    let written_bytes = fs::read(the_file(written.path())).expect("the file reads");
    assert_eq!(reference_bytes, written_bytes);
}

#[test]
fn the_file_says_the_database_s_own_rule_and_not_the_rows_own() {
    let promised = Scratch::new("promised");
    let derived = Scratch::new("derived");
    let written = Scratch::new("written");

    // **The reference the database writes**: the rows, straight to the store,
    // with the database's own optionality.
    {
        let store =
            columnar_store::Store::open(promised.path().join("tables")).expect("the store opens");
        store
            .append_rows(
                "t",
                &chunks_without_nulls(),
                &optionality(),
                writer_properties(&["row_id"]),
            )
            .expect("the rows are written");
    }

    // **The reference the store used to write**: the same rows, with the
    // rows' own optionality — which says the `name` may not hold a NULL,
    // because this batch happens to hold none.
    {
        let store =
            columnar_store::Store::open(derived.path().join("tables")).expect("the store opens");
        let chunks = chunks_without_nulls();
        store
            .append_rows(
                "t",
                &chunks,
                &columnar_store::rows::nullability(&chunks),
                writer_properties(&["row_id"]),
            )
            .expect("the rows are written");
    }

    // **The database**: the same rows, as typed values.
    {
        let mut database = Database::open(written.path()).expect("the database opens");
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows_without_nulls()).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **And the database's file is the database's file, and not the rows' file.**
    let promised_bytes = fs::read(the_file(promised.path())).expect("the promised file reads");
    let derived_bytes = fs::read(the_file(derived.path())).expect("the derived file reads");
    let written_bytes = fs::read(the_file(written.path())).expect("the file reads");
    assert_eq!(
        promised_bytes, written_bytes,
        "the database writes the file its own row model says"
    );
    assert_ne!(derived_bytes, written_bytes, "and not the file the rows' happenstance says");
}

#[test]
fn the_engine_reads_the_database_s_rows() {
    let reference = Scratch::new("engine-reference");
    let written = Scratch::new("engine-written");

    {
        let store =
            columnar_store::Store::open(reference.path().join("tables")).expect("the store opens");
        store
            .append_rows("t", &reference_chunks(), &optionality(), writer_properties(&["row_id"]))
            .expect("the rows are written");
    }
    {
        let mut database = Database::open(written.path()).expect("the database opens");
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows()).expect("the rows are accepted");
        table.commit().expect("the rows are written");
    }

    // **The rows, read by the engine, are the same rows.**
    let reference_rows = rows_of(&the_file(reference.path()));
    let written_rows = rows_of(&the_file(written.path()));
    assert_eq!(reference_rows.chunk("id"), written_rows.chunk("id"));
    assert_eq!(reference_rows.chunk("name"), written_rows.chunk("name"));
    assert_eq!(written_rows.len(), 3);
}

#[test]
fn the_rows_wait_for_the_commit() {
    let scratch = Scratch::new("waiting");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows()).expect("the rows are accepted");
        assert_eq!(table.pending_rows(), 3);
        assert!(
            !the_file(scratch.path()).exists(),
            "an insert is not a commit, and a commit is a file"
        );
        table.commit().expect("the rows are written");
    }

    assert!(the_file(scratch.path()).exists(), "the commit is the file");
    let state =
        database.store().table("t").expect("the store answers").expect("the table is there");
    assert_eq!(state.rows(), 3);
    assert_eq!(state.files().len(), 1);
}

#[test]
fn the_rows_survive_the_handle() {
    let scratch = Scratch::new("survive");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        table.insert(&rows()).expect("the rows are accepted");
    }

    // **The rows are the database's, not the handle's.**
    assert_eq!(database.pending_rows("t"), 3);
    database.commit("t").expect("the rows are written");
    assert_eq!(database.pending_rows("t"), 0);
    let state =
        database.store().table("t").expect("the store answers").expect("the table is there");
    assert_eq!(state.rows(), 3);
}

#[test]
fn the_buffer_is_written_when_it_reaches_the_grid() {
    let scratch = Scratch::new("grid");

    let grid = columnar_store::BATCH_ROWS;
    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");
        let rows: Vec<Row> = (0..grid)
            .map(|index| {
                let id = i64::try_from(index).expect("the row fits an int64");
                Row::filled(vec![ScalarValue::Int64(id), ScalarValue::Utf8("x".to_owned())])
            })
            .collect();
        table.insert(&rows).expect("the rows are accepted");
    }

    // **The buffer reached the grid, so it was written** -- one file, one grid
    // of rows, and the file count grows with the data rather than with the
    // number of inserts.
    let state =
        database.store().table("t").expect("the store answers").expect("the table is there");
    assert_eq!(state.files().len(), 1);
    assert_eq!(state.rows(), grid as u64);
    assert_eq!(database.pending_rows("t"), 0);
}

#[test]
fn a_row_that_does_not_fit_the_schema_is_refused() {
    let scratch = Scratch::new("refused");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    {
        let mut table = database.create_table("t", schema()).expect("the table is created");

        let error = table
            .insert(&[Row::filled(vec![ScalarValue::Int64(1)])])
            .expect_err("the row is refused");
        assert!(
            error.to_string().contains("the row has 1 values, and the table has 2 columns"),
            "{error}"
        );

        let error = table
            .insert(&[Row::filled(vec![ScalarValue::Utf8("x".to_owned()), ScalarValue::Int64(1)])])
            .expect_err("the value is refused");
        assert!(
            error
                .to_string()
                .contains("the value for the column id is a utf8, and the column is an int64"),
            "{error}"
        );

        let error = table
            .insert(&[Row::new(vec![None, Some(ScalarValue::Utf8("x".to_owned()))])])
            .expect_err("the null is refused");
        assert!(error.to_string().contains("the column id may not hold a NULL"), "{error}");
    }

    // **And a refused row is not buffered**, so it cannot be written later.
    assert_eq!(database.pending_rows("t"), 0);
    assert!(!the_file(scratch.path()).exists());
}

#[test]
fn a_commit_with_nothing_waiting_is_not_an_error() {
    let scratch = Scratch::new("empty-commit");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    database.create_table("t", schema()).expect("the table is created");
    database.commit("t").expect("a commit with nothing waiting is not an error");
    assert!(!the_file(scratch.path()).exists());
}

#[test]
fn a_table_that_is_not_there_is_refused() {
    let scratch = Scratch::new("unknown");

    let mut database = Database::open(scratch.path()).expect("the database opens");
    let error = database.commit("nope").expect_err("the table is refused");
    assert!(error.to_string().contains("the database does not hold a table named nope"), "{error}");
}
