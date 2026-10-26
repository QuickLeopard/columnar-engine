//! A Parquet file the engine reads lazily.
//!
//! # Why this exists
//!
//! [`Dataset::read_parquet`](crate::Dataset::read_parquet) decodes **every**
//! column of the file before any query runs. On the TPC-H `lineitem` file
//! that is 231.6 MiB of compressed bytes and 404.5 MiB of raw bytes, of
//! which the Q6 predicate set needs 11.5 MiB and 11.7 MiB: **2.9%**.
//! `l_comment` alone is half of everything the eager reader decodes, and Q6
//! never reads it.
//!
//! ADR-0008 recorded that cost as a deliberate choice for the first
//! increment, with a clause to revisit it. This module is the revisit.
//!
//! # What a source is
//!
//! A [`ParquetSource`] holds a path and the file's *metadata*. Opening one
//! reads the footer and nothing else: no row is decoded, and no column's
//! data is fetched. The query then decides what it needs, and the source
//! reads exactly that.
//!
//! ```text
//! open   ->  schema, row count, row-group count        (the footer)
//! read   ->  the columns the plan names, with the plan's
//!            predicates pushed into the reader            (the data)
//! ```
//!
//! # What is read
//!
//! Two things decide the read:
//!
//! - **Projection.** The reader is handed a [`ProjectionMask`] over the
//!   file's root columns, so the bytes of the columns outside it are never
//!   fetched from the file -- not read, not decompressed, not decoded.
//! - **Pushdown.** The plan's predicates are evaluated by the engine over the
//!   decoded batch, and the rows that fail them are dropped during the scan
//!   rather than after it. The reader is given **no** row filter: a row
//!   filter is a list of row ranges, and the list is longer than the mask.
//!   See [ADR-0015](../../../docs/adr/0015-the-engine-keeps-the-mask.md).
//!
//! The pushdown's semantics are the engine's, not Arrow's, and that is the
//! whole of [`crate::pushdown`].

use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::datatypes::{DataType as ArrowType, SchemaRef};
use columnar_core::DataType;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;

use crate::dataset::{decode_batches_filtered, Dataset, DatasetError};
use crate::plan::ColumnSchema;
use crate::pushdown::PushdownPlan;
use columnar_kernels::Flavor;

/// A column of a [`ParquetSource`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceColumn {
    /// The column's name.
    pub name: String,
    /// The engine's type for the column.
    ///
    /// A dictionary-encoded UTF-8 column is a `utf8` column to the engine:
    /// the encoding is the chunk's business, not the plan's.
    pub data_type: DataType,
    /// The file's Arrow type for the column.
    ///
    /// The pushdown needs this, because it is the pushdown that has to
    /// evaluate an Arrow array.
    pub arrow_type: ArrowType,
}

/// The number of rows the Parquet reader hands over at a time.
///
/// The reader's own default is 1024 rows, which for a six-million-row file
/// means 5,860 batches. Every batch costs a little: the readers are set up,
/// the arrays are allocated, and the engine's builders are extended.
///
/// **This constant is measured for the lazy shape**, and the lazy shape reads
/// one to three columns. It was 8,192 rows for a long time, and it was
/// measured -- in spec 002, for a two-column lazy read. But the scan's shape
/// has changed **three times** since that measurement: the pushdown became
/// the engine's own kernel, the mask stopped going to the reader as a list of
/// row ranges, and the scan stopped filtering the whole batch and copying the
/// survivors twice. A constant measured for a shape that no longer exists is
/// not a measured constant for the shape that does.
///
/// Measured for the shape it is used on — the engine's own lazy scan, end to
/// end, at each size, and then the two candidates against each other with the
/// order alternating so that a machine drifting through the run cannot bias
/// the result — 65,536 rows is **10–12% faster** than 8,192. The sweep is
/// `crates/columnar-engine/examples/lazy_batch_size.rs`, and it uses
/// [`ParquetSource::read_with_batch_rows`] to change nothing but the
/// constant.
///
/// Larger is not better: at 262,144 rows the batches get big enough that the
/// allocation and the copy cost more than the per-batch overhead they save.
/// The engine's own operator batch size is a separate thing, and unrelated.
pub const READER_BATCH_ROWS: usize = 65_536;

/// A Parquet file the engine can read without decoding it.
#[derive(Debug, Clone)]
pub struct ParquetSource {
    /// Where the file is.
    path: PathBuf,
    /// The file's Arrow schema.
    schema: SchemaRef,
    /// The file's columns, in schema order.
    columns: Vec<SourceColumn>,
    /// How many rows the file holds.
    rows: usize,
    /// How many row groups the file has.
    row_groups: usize,
}

impl ParquetSource {
    /// Opens a Parquet file, reading its metadata and nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Io`] if the file cannot be opened, and
    /// [`DatasetError::Parquet`] if it is not a Parquet file the reader
    /// understands, or holds a column type the engine does not support.
    pub fn open(path: &Path) -> Result<Self, DatasetError> {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|error| DatasetError::Parquet(error.to_string()))?;

        let schema = builder.schema().clone();
        let metadata = builder.metadata();
        let rows = usize::try_from(metadata.file_metadata().num_rows()).unwrap_or(0);
        let row_groups = metadata.num_row_groups();

        let columns = schema
            .fields()
            .iter()
            .map(|field| {
                Ok(SourceColumn {
                    name: field.name().clone(),
                    data_type: engine_type(field.data_type())?,
                    arrow_type: field.data_type().clone(),
                })
            })
            .collect::<Result<Vec<SourceColumn>, DatasetError>>()?;

        Ok(Self { path: path.to_path_buf(), schema, columns, rows, row_groups })
    }

    /// Where the file is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many rows the file holds.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// How many row groups the file has.
    #[must_use]
    pub fn row_groups(&self) -> usize {
        self.row_groups
    }

    /// The file's columns, in schema order.
    #[must_use]
    pub fn columns(&self) -> &[SourceColumn] {
        &self.columns
    }

    /// The index of the column called `name`.
    #[must_use]
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column.name == name)
    }

    /// The names of the columns at `indices`, in the order given.
    #[must_use]
    pub fn column_names(&self, indices: &[usize]) -> Vec<String> {
        indices
            .iter()
            .filter_map(|&index| self.columns.get(index).map(|column| column.name.clone()))
            .collect()
    }

    /// Reads the given columns, with the given predicates pushed into the
    /// reader.
    ///
    /// `columns` names the file's root columns to read, in ascending order.
    /// The columns outside it are never fetched from the file.
    ///
    /// The dataset that comes back holds the columns in the order given, and
    /// only the rows that survived the pushed-down predicates.
    ///
    /// `threads` is how many threads to read with. One thread is the same
    /// path this method always took. More than one splits the file's **row
    /// groups** across the threads -- a row group is decoded independently of
    /// every other one -- and merges the parts afterwards.
    ///
    /// The merged dataset holds the file's rows in the file's order, whatever
    /// order the threads finish in. That is a correctness requirement and
    /// not a preference: `SUM` is not associative, so a dataset ordered by
    /// completion would make the engine's answer depend on the scheduler.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Io`] if the file cannot be read, and
    /// [`DatasetError::Parquet`] if it is not one the reader understands.
    pub fn read(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
    ) -> Result<Dataset, DatasetError> {
        self.read_with_batch_rows(columns, pushdown, flavor, threads, READER_BATCH_ROWS)
    }

    /// Reads the file with a given reader batch size.
    ///
    /// This is the **measurement seam**. The lazy path's batch size is a
    /// tuning constant, and the rule for one is that it must be measured for
    /// the shape of the work it is used on: the eager path has its own, and
    /// spec 008 measured it for sixteen columns. The lazy path reads two or
    /// three, so its constant is measured separately -- by
    /// `crates/columnar-engine/examples/lazy_batch_size.rs`, which uses this
    /// method to sweep the constant and nothing else.
    pub fn read_with_batch_rows(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
        batch_rows: usize,
    ) -> Result<Dataset, DatasetError> {
        let workers = self.worker_count(threads);
        if workers == 1 {
            return self.read_row_groups(columns, pushdown, flavor, None, batch_rows);
        }
        self.read_parallel(columns, pushdown, flavor, workers, self.row_groups, batch_rows)
    }

    /// The file's row group count.
    ///
    /// This is the **cap** on the read's parallelism: the read splits the
    /// file's row groups across the threads, so a file with six row groups
    /// cannot be read by more than six threads at a time, however many cores
    /// the machine has. A reader looking at a flat curve past this point
    /// should know that the ceiling is the file's, not the engine's.
    #[must_use]
    pub fn worker_count(&self, threads: usize) -> usize {
        threads.min(self.row_groups).max(1)
    }

    /// Reads the file with several threads.
    ///
    /// The row groups are split into contiguous blocks, one per thread, and
    /// the parts are merged **in block order**. Contiguous blocks are what
    /// makes the merge order the file's order without any sorting.
    fn read_parallel(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        workers: usize,
        row_groups: usize,
        batch_rows: usize,
    ) -> Result<Dataset, DatasetError> {
        let per_worker = row_groups.div_ceil(workers);
        let blocks: Vec<Vec<usize>> = (0..workers)
            .map(|worker| {
                let start = worker * per_worker;
                let end = (start + per_worker).min(row_groups);
                (start..end).collect()
            })
            .filter(|block: &Vec<usize>| !block.is_empty())
            .collect();

        // Each thread writes its own part, and the parts are joined **in
        // block order**. Joining in order is what makes the merged dataset
        // hold the file's rows in the file's order, without any sorting and
        // without any shared mutable state.
        let parts = std::thread::scope(|scope| -> Result<Vec<Dataset>, DatasetError> {
            let handles: Vec<_> = blocks
                .iter()
                .map(|block| {
                    let source = self;
                    scope.spawn(move || {
                        source.read_row_groups(
                            columns,
                            pushdown,
                            flavor,
                            Some(block.clone()),
                            batch_rows,
                        )
                    })
                })
                .collect();

            let mut parts = Vec::with_capacity(handles.len());
            for handle in handles {
                let part = handle
                    .join()
                    .map_err(|_| DatasetError::Parquet("a read thread panicked".to_owned()))?;
                parts.push(part?);
            }
            Ok(parts)
        })?;
        crate::dataset::merge(parts)
    }

    /// Reads the given row groups -- or all of them, when `row_groups` is
    /// `None`.
    fn read_row_groups(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        row_groups: Option<Vec<usize>>,
        batch_rows: usize,
    ) -> Result<Dataset, DatasetError> {
        let file = File::open(&self.path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|error| DatasetError::Parquet(error.to_string()))?;

        // The reader projects the columns the query **materialises** and the
        // columns the **predicates** read. The engine applies the predicates
        // itself, over the reader's arrays, and keeps the mask: a row filter
        // would turn that mask into a range list, and the reader would apply
        // the list one range at a time. See ADR-0015.
        let projection = scan_projection(columns, pushdown);

        // The schema descriptor is needed to build the projection mask, and it
        // is borrowed from the builder -- which `with_projection` consumes. The
        // borrow therefore ends before the builder is consumed.
        let mask = ProjectionMask::roots(builder.parquet_schema(), projection.iter().copied());

        let mut builder = builder.with_projection(mask).with_batch_size(batch_rows);
        if let Some(row_groups) = row_groups {
            builder = builder.with_row_groups(row_groups);
        }

        let reader = builder.build().map_err(|error| DatasetError::Parquet(error.to_string()))?;
        decode_batches_filtered(reader, &self.schema, columns, &projection, pushdown, flavor)
    }
}

/// The columns the scan reads from the file.
///
/// The columns the query **materialises**, and the columns the **predicates**
/// read. Both are needed: the engine applies the predicates over the reader's
/// arrays, so the predicates' columns have to be read -- but they are dropped
/// afterwards, because the query did not ask for them.
///
/// The list is in ascending root-index order, which is the order the reader
/// hands the columns over in.
fn scan_projection(columns: &[usize], pushdown: &PushdownPlan) -> Vec<usize> {
    let mut projection = columns.to_vec();
    projection.extend(pushdown.columns());
    projection.sort_unstable();
    projection.dedup();
    projection
}

impl ColumnSchema for ParquetSource {
    fn column_count(&self) -> usize {
        self.columns.len()
    }

    fn column_name(&self, index: usize) -> &str {
        self.columns.get(index).map_or("", |column| column.name.as_str())
    }

    fn column_type(&self, index: usize) -> DataType {
        self.columns.get(index).map_or(DataType::Int64, |column| column.data_type)
    }
}

/// The engine's type for an Arrow type.
fn engine_type(arrow_type: &ArrowType) -> Result<DataType, DatasetError> {
    match arrow_type {
        ArrowType::Int64 => Ok(DataType::Int64),
        ArrowType::Float64 => Ok(DataType::Float64),
        ArrowType::Utf8 | ArrowType::LargeUtf8 => Ok(DataType::Utf8),
        ArrowType::Dictionary(_, value) => match value.as_ref() {
            ArrowType::Utf8 | ArrowType::LargeUtf8 => Ok(DataType::Utf8),
            other => Err(DatasetError::UnsupportedType(format!(
                "a dictionary column over {other} is not supported"
            ))),
        },
        other => Err(DatasetError::UnsupportedType(format!("{other}"))),
    }
}
