//! The nullability a file set's files may disagree about.
//!
//! **The files of one table have to agree about their columns, their order, their types and their
//! storage, and the engine's file-set check says so.** **And the check does not compare the
//! nullability**, and this test is the reason it does not have to: a file set whose files disagree
//! about a column's optionality **reads correctly**, because the engine's own vocabulary has no
//! nullability in it — the engine's chunk is a dense array of values and a mask that says which of
//! them are real, and the mask is the file's own.
//!
//! **So the disagreement is a fact about the bytes and not about the answer.** The store derives a
//! file's optionality from the rows in it, so two files of one table can carry different
//! optionality for the same column; and the engine reads them as one table anyway, because the
//! first file's schema is the table's and every file is read with its own.
//!
//! **And this is what the check's silence means.** It is not an oversight left for a later
//! increment to find: it is the engine's vocabulary saying that a column's optionality is not the
//! engine's business. The database's business is different — the database's files have to agree
//! with the table's schema — and that is the database's own rule.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_engine::{ExecOptions, FileSetSource, QueryPlan};
use parquet::arrow::ArrowWriter;

/// A number that is different every time it is called.
fn unique() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A directory under the system's temporary directory, removed when it is dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A directory unique to this test.
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("columnar-engine-nullability-{name}-{}", unique()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the directory is created");
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// Writes a file with one integer column and one string column.
///
/// **The string column's optionality is the caller's**, and it is the whole of the experiment: two
/// files that hold the same column with a different optionality are two files a table can hold.
fn write_file(path: &Path, ids: Vec<i64>, names: Vec<Option<String>>, nullable: bool) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, nullable),
    ]));
    let ids: ArrayRef = Arc::new(Int64Array::from(ids));
    let names: ArrayRef = Arc::new(StringArray::from(names));
    let batch = RecordBatch::try_new(schema.clone(), vec![ids, names]).expect("the batch is built");
    let file = File::create(path).expect("the file is created");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer is built");
    writer.write(&batch).expect("the batch is written");
    writer.close().expect("the writer is closed");
}

/// The rows of a file set, read by the engine.
fn read(paths: &[PathBuf]) -> columnar_engine::Dataset {
    let source = FileSetSource::open(paths).expect("the file set opens");
    source.execute_rows(&QueryPlan::scan_all(), &ExecOptions::default()).expect("the rows are read")
}

#[test]
fn a_file_set_whose_files_disagree_about_nullability_reads_correctly() {
    let scratch = Scratch::new("disagree");
    let required = scratch.path.join("required.parquet");
    let optional = scratch.path.join("optional.parquet");

    // **The first file holds no NULL**, so the store would write its string
    // column REQUIRED. **The second holds one**, so the store would write the
    // same column OPTIONAL. **The two files disagree, and the engine's own
    // file-set check does not see it.**
    write_file(&required, vec![1, 2], vec![Some("one".to_owned()), Some("two".to_owned())], false);
    write_file(&optional, vec![3], vec![None], true);

    let paths = vec![required, optional];
    let dataset = read(&paths);

    // **And the rows are the rows**: three of them, and the third has no name.
    assert_eq!(dataset.len(), 3, "the file set holds every row");
    let names = dataset.chunk("name").expect("the set holds the name column");
    let values: Vec<Option<String>> = match &names.encoding {
        columnar_engine::Encoding::Utf8 { values } => {
            values.iter().map(|value| Some(value.clone())).collect()
        }
        other => panic!("the name column is not a string column: {other:?}"),
    };
    assert_eq!(
        values,
        vec![Some("one".to_owned()), Some("two".to_owned()), Some(String::new()),],
        "the rows come back as they were written"
    );
}
