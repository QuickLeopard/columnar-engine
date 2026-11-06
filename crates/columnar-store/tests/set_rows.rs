//! The set's rows: a store's table, read by the engine and written back.
//!
//! # What this file is
//!
//! The store writes a table as a **set of files**, and the engine reads that set as one table. This file
//! closes the loop between them, and it says four things:
//!
//! - **The set's rows are handed out.** `FileSetSource::execute_rows` gives a table's rows back, in the
//!   table's order, as the engine's own vocabulary.
//! - **The round trip is exact.** The rows the engine reads out of a table, written back by the store,
//!   are **the file they came from**, byte for byte -- not an equivalent file, the same bytes.
//! - **The set makes no difference.** A table of one file and a table of two files hold the same rows, and
//!   the rows they hand out are the same rows, in the same order, and they write the same file.
//! - **An aggregation is refused.** The aggregate folds the rows into groups, and there is nothing left to
//!   write -- on the set's path exactly as on the other two.
//!
//! # What this file costs
//!
//! **The engine is a dev-dependency here**, as it is in `engine_rows.rs` and in `glue.rs`, and the edge is
//! **path-only** for the same reason: a versioned dev-dependency would make `cargo package
//! -p columnar-store` wait for the engine to be on crates.io, and the store is published before the engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use arrow::array::{ArrayRef, RecordBatchReader};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use columnar_engine::{Dataset, ExecOptions, FileSetSource, QueryPlan};
use columnar_store::rows;
use columnar_store::{writer_properties, Store, BATCH_ROWS, ROW_GROUP_ROWS};
use columnar_testdata::schema::COMMENT_COLUMN;
use columnar_testdata::writer::write_dataset;
use columnar_testdata::DatasetSpec;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// The rows in the test's dataset: a few batches and one row group.
const ROWS: u64 = 200_000;

/// The generator's file, written once for the whole test binary.
fn generated() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join("columnar-store-set-rows.parquet");
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

/// The file's own schema, read from the file.
fn source_schema(path: &Path) -> SchemaRef {
    let file = fs::File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    builder.build().expect("the reader").schema()
}

/// The file's own rows, read once, in the store's grid.
fn read_source(path: &Path) -> Vec<RecordBatch> {
    let file = fs::File::open(path).expect("the file opens");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the builder");
    let reader = builder.with_batch_size(BATCH_ROWS).build().expect("the reader");
    reader.map(|batch| batch.expect("the batch")).collect()
}

/// The plan the round trip runs: **every row, every column**.
///
/// The projection is empty, so the plan's rows carry every column of the table, and there is no filter, so
/// every row survives. **The rows are the table's rows.**
fn plan() -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: None,
        join: None,
        sort: None,
    }
}

/// The batches the arrays make, on the store's grid.
fn on_the_grid(arrays: &[ArrayRef], schema: &SchemaRef, rows: usize) -> Vec<RecordBatch> {
    let mut batches = Vec::new();
    let mut start = 0;
    while start < rows {
        let len = BATCH_ROWS.min(rows - start);
        let columns = arrays.iter().map(|array| array.slice(start, len)).collect();
        batches.push(RecordBatch::try_new(schema.clone(), columns).expect("the batch is made"));
        start += len;
    }
    batches
}

/// The file's data region: everything before the file's metadata.
///
/// A Parquet file ends with the metadata, its length, and the magic. **The metadata is where the Arrow
/// schema lives**, and the Arrow schema is the one thing the engine's rows cannot carry -- the rows say
/// which rows are NULL and not whether a column may be. **So the data region is what the round trip has
/// to reproduce exactly**, and the metadata is what the store's own rule changes.
fn data_region(path: &Path) -> Vec<u8> {
    let bytes = fs::read(path).expect("the file reads");
    let (metadata_start, _) = split_metadata(&bytes);
    bytes.get(..metadata_start).expect("the data region is there").to_vec()
}

/// The file's metadata, and so its Arrow schema.
fn metadata(path: &Path) -> Vec<u8> {
    let bytes = fs::read(path).expect("the file reads");
    let (metadata_start, metadata_len) = split_metadata(&bytes);
    bytes
        .get(metadata_start..metadata_start + metadata_len)
        .expect("the metadata is there")
        .to_vec()
}

/// Where the file's metadata starts, and how long it is.
///
/// A Parquet file ends with the metadata, its length as four little-endian bytes, and the four-byte magic.
fn split_metadata(bytes: &[u8]) -> (usize, usize) {
    assert!(bytes.len() > 8, "the file is long enough to hold a footer");
    let length = bytes.get(bytes.len() - 8..bytes.len() - 4).expect("the length is there");
    let metadata_len = u32::from_le_bytes(length.try_into().expect("four bytes")) as usize;
    (bytes.len() - 8 - metadata_len, metadata_len)
}

/// Writes the engine's rows back into the store under the file's own schema.
///
/// This is the round trip: the table's paths go to the set, the set's rows come out, and the store writes
/// them. **The schema is the file's own**, so the file that comes out is compared with the file the rows
/// came from -- the store's own nullability rule is the subject of `engine_rows.rs`, and it is not what
/// this file is about.
fn round_trip(store: &Store, table_name: &str, schema: &SchemaRef) -> PathBuf {
    let table = store.table(table_name).expect("the table is there").expect("the table is there");
    let set = FileSetSource::open(&table.paths()).expect("the set opens");
    let rows = set.execute_rows(&plan(), &ExecOptions::default()).expect("the rows come out");
    let arrays = rows::arrays(&rows.chunks).expect("the chunks become arrays");
    let batches = on_the_grid(&arrays, schema, rows.rows);
    let manifest = store
        .append_with("round-trip", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let file = manifest.files.last().expect("the manifest names a file").name.clone();
    store.file_path("round-trip", &file)
}

/// Whether two files are equal, byte for byte.
fn files_identical(left: &Path, right: &Path) -> bool {
    fs::read(left).expect("the left file reads") == fs::read(right).expect("the right file reads")
}

/// Writes the generated file's rows into the store, as one table of one file and one of two.
fn write_tables(store: &Store, batches: &[RecordBatch]) {
    store
        .append_with("one-file", batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the one-file table is written");
    let split = 2.min(batches.len());
    let first = batches.get(..split).expect("the first batches are there");
    let second = batches.get(split..).expect("the second batches are there");
    store
        .append_with("two-files", first, writer_properties(&[COMMENT_COLUMN]))
        .expect("the first file is written");
    store
        .append_with("two-files", second, writer_properties(&[COMMENT_COLUMN]))
        .expect("the second file is written");
}

#[test]
fn the_round_trip_is_the_file() {
    let generated = generated();
    // The rows the engine hands out are **the rows that were read**, so they keep the file's own
    // encodings: a dictionary column is a dictionary column. **So the schema that describes them is the
    // file's own schema**, and the file that comes out of the round trip is the file they came from.
    let schema = source_schema(generated);
    let store = Store::open(scratch("set-rows-exact")).expect("the store opens");
    let batches = read_source(generated);
    write_tables(&store, &batches);

    // The store's own table holds the generated file's rows, and the round trip writes them back.
    let written = round_trip(&store, "one-file", &schema);

    // The data is the file's data, byte for byte.
    assert_eq!(
        data_region(generated),
        data_region(&written),
        "the rows the engine read out of the table do not write the data the file holds"
    );
    // And the metadata is the file's metadata too: the rows are the rows that were read, so they keep
    // the dictionary, and the footer records the dictionary the file holds.
    assert_eq!(
        metadata(generated),
        metadata(&written),
        "the round trip's metadata is not the file's metadata: the dictionary did not survive the rows"
    );
    // So the file is the file, byte for byte: not the data region alone, the whole file.
    assert!(
        files_identical(generated, &written),
        "the round trip's file is not the file the rows came from, byte for byte"
    );
}

#[test]
fn the_rows_keep_the_dictionary() {
    let generated = generated();
    let store = Store::open(scratch("set-rows-dictionary")).expect("the store opens");
    let batches = read_source(generated);
    write_tables(&store, &batches);

    // The rows the set hands out are the rows that were read, so the dictionary is still there.
    let table = store.table("one-file").expect("the table is there").expect("the table is there");
    let set = FileSetSource::open(&table.paths()).expect("the set opens");
    let rows = set.execute_rows(&plan(), &ExecOptions::default()).expect("the rows come out");
    let dictionaries = rows
        .chunks
        .iter()
        .filter(|chunk| matches!(chunk.encoding, columnar_core::Encoding::Utf8Dict { .. }))
        .count();
    assert!(dictionaries > 0, "the rows came out plain: the dictionary was materialised away");
}

#[test]
fn the_sets_rows_are_the_files_rows() {
    let generated = generated();
    // The rows the engine hands out are **the rows that were read**, so they keep the file's own
    // encodings: a dictionary column is a dictionary column. **So the schema that describes them is the
    // file's own schema**, and the file that comes out of the round trip is the file they came from.
    let schema = source_schema(generated);
    let store = Store::open(scratch("set-rows-set")).expect("the store opens");
    let batches = read_source(generated);
    write_tables(&store, &batches);

    // The one file's rows, and the two files' rows.
    let one = store.table("one-file").expect("the table is there").expect("the table is there");
    let two = store.table("two-files").expect("the table is there").expect("the table is there");
    assert_eq!(one.files().len(), 1, "the first table holds one file");
    assert_eq!(two.files().len(), 2, "the second table holds two files");

    let one_set = FileSetSource::open(&one.paths()).expect("the set opens");
    let two_set = FileSetSource::open(&two.paths()).expect("the set opens");
    let one_rows =
        one_set.execute_rows(&plan(), &ExecOptions::default()).expect("the rows come out");
    let two_rows =
        two_set.execute_rows(&plan(), &ExecOptions::default()).expect("the rows come out");

    assert_eq!(one_rows.rows, two_rows.rows, "the two sets hold the same rows");
    assert_eq!(
        one_rows.chunks, two_rows.chunks,
        "the two sets' rows are not the same rows, in the same order"
    );

    // And the files they write are the same file.
    let one_written = round_trip(&store, "one-file", &schema);
    let two_written = round_trip(&store, "two-files", &schema);
    assert!(
        files_identical(&one_written, &two_written),
        "the one file's rows and the two files' rows do not write the same file"
    );
}

#[test]
fn a_plan_with_an_aggregate_is_refused() {
    let generated = generated();
    let store = Store::open(scratch("set-rows-aggregate")).expect("the store opens");
    let batches = read_source(generated);
    write_tables(&store, &batches);

    let table = store.table("one-file").expect("the table is there").expect("the table is there");
    let set = FileSetSource::open(&table.paths()).expect("the set opens");
    let mut plan = plan();
    plan.aggregate = Some(columnar_engine::AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![columnar_engine::Aggregate::CountStar],
    });
    let error = set.execute_rows(&plan, &ExecOptions::default()).expect_err("the plan is refused");
    assert!(
        error.to_string().contains("no rows to write"),
        "the refusal does not say why: {error}"
    );
}

#[test]
fn the_rows_are_the_tables_rows() {
    let generated = generated();
    let store = Store::open(scratch("set-rows-count")).expect("the store opens");
    let batches = read_source(generated);
    write_tables(&store, &batches);

    let table = store.table("two-files").expect("the table is there").expect("the table is there");
    let set = FileSetSource::open(&table.paths()).expect("the set opens");
    let rows = set.execute_rows(&plan(), &ExecOptions::default()).expect("the rows come out");
    assert_eq!(rows.rows as u64, table.rows(), "the rows are the table's rows");
    assert_eq!(rows.rows as u64, ROWS, "the rows are every row");
    assert_eq!(rows.chunks.len(), 16, "the rows carry every column");
    let _ = Dataset::read_parquet(generated).expect("the engine reads the file");
}
