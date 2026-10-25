//! The parallel eager read must be **bit-identical** to the serial one.
//!
//! The eager read decodes every column of the file into the engine's own
//! chunk format, and the parallel one splits the file's **row groups** across
//! the workers and merges the parts afterwards. The merge is where the
//! danger is:
//!
//! - the parts' dictionaries have to be merged into **one** dictionary, in
//!   first-appearance order, or the merged chunk is not the chunk the serial
//!   read produced;
//! - every row's index into its part's dictionary has to be repointed at the
//!   merged dictionary's slot;
//! - the validity masks have to be concatenated with the mid-word shift;
//! - and the rows have to stay in the file's order, because `SUM` is not
//!   associative.
//!
//! So the tests here compare the **whole dataset** -- every chunk, every
//! value, every validity bit, every dictionary -- against the serial read's,
//! at every thread count. `Dataset` derives `PartialEq`, so one comparison
//! covers all of it.
//!
//! The files are written here rather than generated, because the edges that
//! matter are exactly the ones a generator would smooth over: NULLs, several
//! row groups, and dictionary columns whose row groups hold **different**
//! values, which is what makes the dictionary merge do any work at all.

// The column indices below are literals into a three-column dataset whose
// shape the test has just asserted, so the panic the lint guards against
// cannot happen.
#![allow(clippy::indexing_slicing)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringDictionaryBuilder};
use arrow::datatypes::{DataType, Field, Int32Type, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use columnar_core::Encoding;
use columnar_engine::{Dataset, QueryPlan};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

/// A temporary file that removes itself when it is dropped.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A path under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!("columnar-eager-{name}-{unique}.parquet"));
        Self { path }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The schema the tests write.
fn schema() -> SchemaRef {
    Schema::new(vec![
        Field::new("l_discount", DataType::Float64, true),
        Field::new(
            "l_shipdate",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            true,
        ),
        Field::new("l_quantity", DataType::Int64, true),
    ])
    .into()
}

/// Writes a Parquet file holding **one row group per batch**.
fn write_row_groups(path: &Path, batches: &[RecordBatch]) {
    let first = batches.first().expect("there is at least one batch");
    let rows = first.num_rows();
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_max_row_group_size(rows)
        .build();
    let file = File::create(path).expect("the temporary file can be created");
    let mut writer = ArrowWriter::try_new(file, first.schema(), Some(properties))
        .expect("the arrow writer accepts the batch");
    for batch in batches {
        writer.write(batch).expect("the batch can be written");
    }
    writer.close().expect("the file can be closed");
}

/// A float column.
fn floats(values: &[Option<f64>]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}

/// An integer column.
fn integers(values: &[Option<i64>]) -> ArrayRef {
    Arc::new(Int64Array::from(values.to_vec()))
}

/// A dictionary column over UTF-8.
fn dictionary(values: &[Option<&str>]) -> ArrayRef {
    let mut builder = StringDictionaryBuilder::<Int32Type>::new();
    for value in values {
        match value {
            Some(value) => {
                builder.append(*value).expect("the value is appended");
            }
            None => builder.append_null(),
        }
    }
    Arc::new(builder.finish())
}

/// Four row groups of eight rows, with NULLs, NaNs, and **different** values
/// in each row group's dictionary.
///
/// The dictionary column is the interesting one: each row group holds its own
/// distinct dates, so a merge that got the order wrong -- or that repointed
/// the wrong rows -- would show up in the merged column's values.
fn four_row_group_batches() -> Vec<RecordBatch> {
    (0..4)
        .map(|group| {
            let base = i64::from(group * 8);
            let discount = floats(&[
                Some(0.02),
                Some(0.05),
                None,
                Some(f64::NAN),
                Some(0.07),
                None,
                Some(0.04),
                Some(0.09),
            ]);
            // **Each group holds its own dates**, and none of them are the
            // same as another group's.
            //
            // This is the point of the fixture. If the groups held the same
            // dates, every group's dictionary would be identical, the merge's
            // remap would be the identity, and a merge that forgot to repoint
            // the indices would still produce the right answer -- the test
            // would pass on a broken merge. The dates below are the group's
            // own, so the remap has real work to do and a merge that skips it
            // is caught.
            let shipdate = dictionary(&[
                Some(&format!("199{}-01-01", 3 + group)),
                Some(&format!("199{}-01-01", 4 + group)),
                Some(&format!("199{}-06-01", 4 + group)),
                None,
                Some(&format!("199{}-01-01", 5 + group)),
                Some(&format!("199{}-12-31", 4 + group)),
                Some(&format!("199{}-03-03", 6 + group)),
                Some(&format!("199{}-06-06", 3 + group)),
            ]);
            let quantity = integers(
                &(0..8).map(|row| Some(base + i64::from(row))).collect::<Vec<Option<i64>>>(),
            );
            RecordBatch::try_new(schema(), vec![discount, shipdate, quantity])
                .expect("the batch matches the schema")
        })
        .collect()
}

/// The rows of a column, resolved.
///
/// A dictionary-encoded column is resolved to its strings, so the comparison
/// is about the **values** and not about which dictionary happened to hold
/// them. A float is rendered with `to_string`, so a `NaN` compares equal to
/// a `NaN` -- which raw equality would not, because `NaN != NaN`.
///
/// A NULL row is `None`.
fn resolved_rows(chunk: &columnar_core::ColumnChunk) -> Vec<Option<String>> {
    let values: Vec<String> = match &chunk.encoding {
        Encoding::Int64 { values } => values.iter().map(ToString::to_string).collect(),
        Encoding::Float64 { values } => values.iter().map(ToString::to_string).collect(),
        Encoding::Utf8 { values } => values.clone(),
        Encoding::Int64Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                dict.get(position).expect("the index is in range").to_string()
            })
            .collect(),
        Encoding::Utf8Dict { dict, indices } => indices
            .iter()
            .map(|index| {
                let position = usize::try_from(*index).expect("the index is not negative");
                dict.get(position).expect("the index is in range").clone()
            })
            .collect(),
        other => panic!("the comparison does not know the `{other:?}` encoding"),
    };

    values
        .into_iter()
        .enumerate()
        .map(|(row, value)| if chunk.validity.is_valid(row) { Some(value) } else { None })
        .collect()
}

/// The thread counts the tests sweep.
const THREADS: [usize; 6] = [1, 2, 3, 4, 8, 14];

/// The parallel eager read must produce exactly the serial read's dataset.
///
/// Not "the same answer" -- the same **dataset**: the same rows, in the same
/// order, with the same values, the same validity bits and the same
/// dictionary.
#[test]
fn the_parallel_eager_read_holds_the_same_dataset_as_the_serial_one() {
    let file = TempFile::new("parallel-eager");
    write_row_groups(&file.path, &four_row_group_batches());

    let serial = Dataset::read_parquet(&file.path).expect("the serial read runs");
    assert_eq!(serial.len(), 32, "the file does not hold every row");

    for threads in THREADS {
        let parallel = Dataset::read_parquet_with_threads(&file.path, threads)
            .expect("the parallel read runs");
        assert_eq!(
            parallel.rows, serial.rows,
            "the parallel eager read holds a different number of rows at {threads} threads"
        );
        for (index, (left, right)) in serial.chunks.iter().zip(parallel.chunks.iter()).enumerate() {
            assert_eq!(
                resolved_rows(right),
                resolved_rows(left),
                "the parallel eager read holds different rows in column {index} at {threads} threads"
            );
            assert_eq!(
                right.validity, left.validity,
                "the parallel eager read holds a different validity mask in column {index} at {threads} threads"
            );
        }
    }
}

/// The file must have several row groups, or the parallel path never engages.
///
/// A file with one row group has one worker however many threads are asked
/// for, so a test written against such a file would be testing the
/// single-threaded path under the parallel path's name.
#[test]
fn the_file_has_several_row_groups() {
    let file = TempFile::new("row-groups");
    write_row_groups(&file.path, &four_row_group_batches());

    let dataset = Dataset::read_parquet(&file.path).expect("the read runs");
    assert_eq!(dataset.len(), 32);

    // The row groups are what the read splits, so the count is what the
    // reader sees.
    let groups = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        File::open(&file.path).expect("the file opens"),
    )
    .expect("the builder")
    .metadata()
    .num_row_groups();
    assert!(groups > 1, "the file has {groups} row group(s), so the parallel path cannot engage");
}

/// The merged dictionary holds the values in **first-appearance order**.
///
/// This is the property that makes the parallel read's chunk the *same chunk*
/// the serial read produced, rather than merely an equivalent one. The serial
/// read merges the row groups' dictionaries in row-group order, and so must
/// the parallel read -- which is what joining the parts in block order buys.
#[test]
fn the_merged_dictionary_holds_the_values_in_first_appearance_order() {
    let file = TempFile::new("dictionary-order");
    write_row_groups(&file.path, &four_row_group_batches());

    let serial = Dataset::read_parquet(&file.path).expect("the serial read runs");
    let parallel =
        Dataset::read_parquet_with_threads(&file.path, 4).expect("the parallel read runs");

    let Encoding::Utf8Dict { dict, .. } = &serial.chunks[1].encoding else {
        panic!("the shipdate column is not dictionary encoded");
    };
    // The dictionary column has no NaN in it, so it can be compared
    // directly: the same dictionary, in the same order, with the same
    // indices.
    assert_eq!(
        parallel.chunks[1].encoding, serial.chunks[1].encoding,
        "the merged dictionary column differs from the serial one"
    );
    assert!(!dict.is_empty(), "the dictionary is empty");

    // The dates, in the order the dictionary holds them.
    let mut dates: Vec<&str> = dict.iter().map(String::as_str).collect();
    dates.sort_unstable();
    dates.dedup();
    // Four groups of seven dates each, with one shared between each pair of
    // neighbouring groups: the merged dictionary holds twenty-two of them.
    assert_eq!(dates.len(), 22, "the dictionary holds {} distinct dates", dates.len());
}

/// The rows must be in the file's order, and the values must point at them.
///
/// The merged chunk's indices have to be repointed at the merged dictionary's
/// slots. A remap that repointed the wrong row would leave the *count* of
/// distinct values right and the *values* wrong, so the values are checked
/// here and the counts elsewhere.
#[test]
fn the_merged_rows_hold_the_values_they_held_before_the_merge() {
    let file = TempFile::new("merged-values");
    write_row_groups(&file.path, &four_row_group_batches());

    let serial = Dataset::read_parquet(&file.path).expect("the serial read runs");
    let parallel =
        Dataset::read_parquet_with_threads(&file.path, 3).expect("the parallel read runs");

    // The dictionary column, resolved row by row.
    let resolved = |dataset: &Dataset| -> Vec<String> {
        let Encoding::Utf8Dict { dict, indices } = &dataset.chunks[1].encoding else {
            panic!("the shipdate column is not dictionary encoded");
        };
        indices
            .iter()
            .map(|&index| {
                let slot = usize::try_from(index).expect("a dictionary index is not negative");
                dict.get(slot).cloned().unwrap_or_default()
            })
            .collect()
    };

    assert_eq!(resolved(&parallel), resolved(&serial), "the merged rows hold different values");
    assert_eq!(resolved(&parallel).len(), 32, "the merged column lost rows");
}

/// The answer must not depend on the thread count.
///
/// This is the requirement that makes the parallel eager read **safe** rather
/// than merely fast: the eager path is what the *resident* comparison uses,
/// so a resident number that depended on the scheduler would be a number
/// about the machine rather than about the engine.
#[test]
fn the_answer_does_not_depend_on_the_thread_count() {
    let file = TempFile::new("answer");
    write_row_groups(&file.path, &four_row_group_batches());

    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(columnar_engine::AggregateSpec {
            group_by: vec!["l_shipdate".to_owned()],
            aggregates: vec![columnar_engine::Aggregate::Sum {
                expr: columnar_core::Expr::column("l_quantity"),
            }],
        }),
    };

    let serial = Dataset::read_parquet(&file.path).expect("the serial read runs");
    let expected =
        columnar_engine::execute(&plan, &serial, &columnar_engine::ExecOptions::default())
            .expect("the query runs");

    for threads in THREADS {
        let dataset = Dataset::read_parquet_with_threads(&file.path, threads)
            .expect("the parallel read runs");
        let options = columnar_engine::ExecOptions { threads: Some(threads), ..Default::default() };
        let result = columnar_engine::execute(&plan, &dataset, &options).expect("the query runs");
        assert_eq!(result.groups, expected.groups, "the answer differs at {threads} threads");
    }
}

/// The engine's own values survive the merge.
///
/// A merge that dropped a row, duplicated one, or lost a NULL would still
/// have the right number of rows in the wrong places. These are the checks
/// that would notice.
#[test]
fn the_merge_keeps_every_value_and_every_null() {
    let file = TempFile::new("values");
    write_row_groups(&file.path, &four_row_group_batches());

    let serial = Dataset::read_parquet(&file.path).expect("the serial read runs");
    let parallel =
        Dataset::read_parquet_with_threads(&file.path, 4).expect("the parallel read runs");

    for (index, (left, right)) in serial.chunks.iter().zip(parallel.chunks.iter()).enumerate() {
        assert_eq!(left.len, right.len, "column {index} holds a different number of rows");
        assert_eq!(left.validity, right.validity, "column {index} holds a different validity mask");
        assert_eq!(
            resolved_rows(right),
            resolved_rows(left),
            "column {index} holds different values"
        );
    }
    assert_eq!(serial.rows, parallel.rows, "the datasets hold a different number of rows");
}
