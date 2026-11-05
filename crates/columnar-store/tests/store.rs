//! The store writes the file the generator wrote, and the commit is a rename.
//!
//! **The acceptance is the strongest one available.** The generator writes a
//! file; the store reads the same rows back and writes them again; and the two
//! files are compared **byte for byte**. So the store's correctness claim is
//! not "it wrote something" but "it wrote the file the generator wrote, and the
//! two are equal".
//!
//! The dataset is generated rather than committed: a binary in the repository
//! would be a blob nobody could review, and a generator nobody could run would
//! be worse.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use arrow::record_batch::RecordBatch;
use columnar_store::{writer_properties, Store, BATCH_ROWS, ROW_GROUP_ROWS};
use columnar_testdata::schema::COMMENT_COLUMN;
use columnar_testdata::writer::write_dataset;
use columnar_testdata::DatasetSpec;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// The rows in the test's dataset: a few batches and one row group.
const ROWS: u64 = 200_000;

/// The generator's file, written once for the whole test binary.
///
/// Generating the dataset takes a second, and several tests read it.
fn generated() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join("columnar-store-generated.parquet");
        let spec = DatasetSpec { scale_factor: 1, seed: 24001, null_rate: 0.0 };
        write_dataset(&spec, ROWS, &path, ROW_GROUP_ROWS).expect("the dataset can be generated");
        path
    })
}

/// A scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("columnar-store-{name}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A file's rows, in the writer's grid.
fn rows_of(path: &Path) -> Vec<RecordBatch> {
    let file = fs::File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let reader = builder.with_batch_size(BATCH_ROWS).build().expect("the reader");
    reader.map(|batch| batch.expect("the batch")).collect()
}

/// Whether two files are equal, byte for byte.
fn files_identical(left: &Path, right: &Path) -> bool {
    fs::read(left).expect("the left file reads") == fs::read(right).expect("the right file reads")
}

#[test]
fn the_store_writes_the_file_the_generator_wrote() {
    let generated = generated();
    let batches = rows_of(generated);
    let store = Store::open(scratch("acceptance")).expect("the store opens");
    let manifest = store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let file = manifest.files.first().expect("the manifest names a file").name.clone();
    let written = store.file_path("lineitem", &file);
    assert!(files_identical(generated, &written), "the store's file is not the generator's file");
}

#[test]
fn the_manifest_names_the_table_and_its_rows() {
    let batches = rows_of(generated());
    let store = Store::open(scratch("manifest")).expect("the store opens");
    let manifest = store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    assert_eq!(manifest.rows, ROWS, "the manifest's rows");
    assert_eq!(manifest.table, "lineitem", "the manifest's table");
    assert_eq!(manifest.files.len(), 1, "the manifest names one file");
    let file = manifest.files.first().expect("the manifest names a file");
    assert_eq!(file.rows, ROWS, "the file's rows");
    let written = store.file_path("lineitem", &file.name);
    assert_eq!(
        fs::metadata(&written).expect("the file is there").len(),
        file.bytes,
        "the manifest's bytes are the file's bytes"
    );
}

#[test]
fn the_catalog_survives_the_process() {
    let dir = scratch("catalog");
    let batches = rows_of(generated());
    {
        let store = Store::open(&dir).expect("the store opens");
        store
            .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
            .expect("the rows are written");
    }
    // A new handle on the same directory: the catalog is on disk.
    let store = Store::open(&dir).expect("the store opens");
    assert_eq!(store.tables().expect("the tables read"), vec!["lineitem".to_owned()]);
    let table = store.table("lineitem").expect("the table reads").expect("the table is there");
    assert_eq!(table.rows(), ROWS, "the table's rows");
}

#[test]
fn a_second_append_adds_a_file_and_keeps_the_first() {
    let batches = rows_of(generated());
    let store = Store::open(scratch("append")).expect("the store opens");
    let first = store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the first append");
    let first_file = first.files.first().expect("a file").name.clone();
    let first_bytes = fs::read(store.file_path("lineitem", &first_file)).expect("the file reads");

    let second = store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the second append");
    assert_eq!(second.files.len(), 2, "the manifest names two files");
    assert_eq!(second.rows, ROWS * 2, "the table's rows");
    assert_eq!(
        fs::read(store.file_path("lineitem", &first_file)).expect("the file reads"),
        first_bytes,
        "the first file is unchanged"
    );
}

#[test]
fn the_grid_is_checked() {
    let batches = rows_of(generated());
    let store = Store::open(scratch("grid")).expect("the store opens");
    // The first batch off the grid, and not the last one: the last may be
    // short, because it is the end of the stream.
    let first = batches.first().expect("the batches are not empty").slice(0, 1024);
    let second = batches.get(1).expect("the batches are not empty").clone();
    let off_grid = vec![first, second];
    let error = store
        .append_with("lineitem", &off_grid, writer_properties(&[]))
        .expect_err("a batch off the grid is refused");
    assert!(error.to_string().contains("grid"), "the error says the grid was broken: {error}");
}

#[test]
fn a_name_that_is_not_a_name_is_refused() {
    let store = Store::open(scratch("name")).expect("the store opens");
    let error =
        store.append("../escape", &[]).expect_err("a name that escapes the store is refused");
    assert!(error.to_string().contains("name"), "the error says the name was refused: {error}");
}

#[test]
fn the_commit_leaves_no_temporary_manifest() {
    let dir = scratch("commit");
    let batches = rows_of(generated());
    let store = Store::open(&dir).expect("the store opens");
    store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let table_dir = store.table_dir("lineitem");
    assert!(
        !table_dir.join("manifest.json.tmp").exists(),
        "the commit left its temporary manifest behind"
    );
    assert!(table_dir.join("manifest.json").is_file(), "the commit left no manifest");
}
