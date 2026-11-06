//! The engine's rows reach a store's table.
//!
//! # What this file is
//!
//! The store's claim is *"fed the same rows, it writes the same file, byte for byte"*, and it has
//! only ever been measured through **Arrow's** rows. This file measures it through the **engine's**
//! rows -- the engine's own vocabulary, a column chunk with an encoding and a validity mask -- and
//! it says four things:
//!
//! - **The rows are written.** The engine's rows reach a table, and the table is committed.
//! - **The conversion is exact.** Under the file's own schema, the engine's rows write the file
//!   the file's own rows write -- **byte for byte**. So the engine's rows are the file's rows, and
//!   the conversion is value for value.
//! - **The grid is the store's.** The engine's rows are one whole column, and the store re-batches
//!   them: every batch but the last is 65,536 rows.
//! - **The schema rule is the store's.** A column that holds no NULL is written **REQUIRED**, and a
//!   column that holds one is written **OPTIONAL** -- because the engine's rows say which rows are
//!   NULL and not whether a column may be. That rule is why the store's own `append_rows` writes a
//!   file that is *not* the file the rows were read from, and the difference is the schema alone.
//!
//! # What this file costs
//!
//! **The engine is a dev-dependency here**, as it is in `glue.rs`, and the edge is **path-only** for
//! the same reason: a versioned dev-dependency would make `cargo package -p columnar-store` wait for
//! the engine to be on crates.io, and the store is published before the engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use arrow::array::RecordBatchReader;
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use columnar_core::{ColumnChunk, Encoding, NullBitmap};
use columnar_engine::Dataset;
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
        let path = std::env::temp_dir().join("columnar-store-engine-rows.parquet");
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

/// Whether two files are equal, byte for byte.
fn files_identical(left: &Path, right: &Path) -> bool {
    fs::read(left).expect("the left file reads") == fs::read(right).expect("the right file reads")
}

#[test]
fn the_engines_rows_write_the_files_own_file() {
    let generated = generated();
    let dataset = Dataset::read_parquet(generated).expect("the engine reads the file");

    // The store's own conversion, under the file's own schema.
    let schema = source_schema(generated);
    let arrays = rows::arrays(&dataset.chunks).expect("the chunks become arrays");
    let mut batches = Vec::new();
    let mut start = 0;
    while start < dataset.rows {
        let len = BATCH_ROWS.min(dataset.rows - start);
        let columns = arrays.iter().map(|array| array.slice(start, len)).collect();
        batches.push(RecordBatch::try_new(schema.clone(), columns).expect("the batch is made"));
        start += len;
    }

    let store = Store::open(scratch("engine-rows")).expect("the store opens");
    let manifest = store
        .append_with("lineitem", &batches, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    let file = manifest.files.first().expect("the manifest names a file").name.clone();
    let written = store.file_path("lineitem", &file);
    assert!(
        files_identical(generated, &written),
        "the engine's rows do not write the file the file's own rows write"
    );
}

#[test]
fn the_engines_rows_are_written_and_committed() {
    let generated = generated();
    let dataset = Dataset::read_parquet(generated).expect("the engine reads the file");
    let store = Store::open(scratch("engine-append")).expect("the store opens");
    let manifest = store
        .append_rows("lineitem", &dataset.chunks, writer_properties(&[COMMENT_COLUMN]))
        .expect("the rows are written");
    assert_eq!(manifest.rows, dataset.rows as u64, "the manifest counts the rows");
    let table = store.table("lineitem").expect("the table is there").expect("the table is there");
    assert_eq!(table.rows(), dataset.rows as u64, "the table counts the rows");
    assert_eq!(table.files().len(), 1, "the table holds one file");
}

#[test]
fn the_grid_is_the_stores() {
    let generated = generated();
    let dataset = Dataset::read_parquet(generated).expect("the engine reads the file");
    let batches = rows::batches(&dataset.chunks).expect("the rows are batched");
    assert!(batches.len() > 1, "the rows are re-batched, not written as one");
    let (last, rest) = batches.split_last().expect("there is a last batch");
    for batch in rest {
        assert_eq!(batch.num_rows(), BATCH_ROWS, "every batch but the last is on the grid");
    }
    assert!(last.num_rows() <= BATCH_ROWS, "the last batch is on the grid too");
    assert_eq!(
        batches.iter().map(RecordBatch::num_rows).sum::<usize>(),
        dataset.rows,
        "the batches hold every row, once"
    );
}

#[test]
fn a_column_with_no_null_is_required_and_one_with_a_null_is_optional() {
    let plain = ColumnChunk {
        name: "k".to_owned(),
        encoding: Encoding::Int64 { values: vec![1, 2, 3] },
        validity: NullBitmap::all_valid(3),
        len: 3,
    };
    let with_null = ColumnChunk {
        name: "k".to_owned(),
        encoding: Encoding::Int64 { values: vec![1, 0, 3] },
        validity: NullBitmap::with_nulls(3, &[1]),
        len: 3,
    };
    let schema = rows::schema(&[plain.clone(), with_null.clone()]);
    assert!(!schema.field(0).is_nullable(), "a column with no NULL is required");
    assert!(schema.field(1).is_nullable(), "a column with a NULL is optional");
}

#[test]
fn a_dictionary_column_is_written_as_a_dictionary() {
    let chunk = ColumnChunk {
        name: "s".to_owned(),
        encoding: Encoding::Utf8Dict {
            dict: vec!["a".to_owned(), "b".to_owned()],
            indices: vec![0, 1, 0],
        },
        validity: NullBitmap::all_valid(3),
        len: 3,
    };
    let schema = rows::schema(&[chunk]);
    let field = schema.field(0);
    assert!(
        matches!(field.data_type(), arrow::datatypes::DataType::Dictionary(_, _)),
        "a dictionary column is written as a dictionary, and not as its values"
    );
}
