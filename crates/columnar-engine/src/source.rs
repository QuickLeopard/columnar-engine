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

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::datatypes::{DataType as ArrowType, SchemaRef};
use columnar_core::{DataType, ScalarValue};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;
use parquet::file::metadata::ParquetMetaData;
use parquet::file::statistics::Statistics;

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

/// The limit on the number of rows the Parquet reader hands over at a time.
///
/// The reader's own default is 1024 rows, which for a six-million-row file
/// means 5,860 batches. Every batch costs a little: the readers are set up,
/// the arrays are allocated, and the engine's builders are extended.
///
/// **This constant is a limit, not a batch size.** The reader's batch size is
/// the largest **divisor of the row group's row count** that fits under it,
/// so that no row group ends with a partial batch. A partial batch is
/// expensive: the parquet crate's reader pays about 0.85 ms for it, once per
/// row group, and that was the whole of the engine's per-row-group cost.
/// See [ADR-0023](../../../docs/adr/0023-the-per-row-group-cost.md).
///
/// The limit bounds the transient batch, so it is also the memory the read
/// holds beyond the dataset it returns.
///
/// The constant itself is measured for the lazy shape, and the lazy shape
/// reads one to three columns. It was 8,192 rows for a long time, and it was
/// measured -- in spec 002, for a two-column lazy read. But the scan's shape
/// has changed **three times** since that measurement: the pushdown became
/// the engine's own kernel, the mask stopped going to the reader as a list of
/// row ranges, and the scan stopped filtering the whole batch and copying the
/// survivors twice. A constant measured for a shape that no longer exists is
/// not a measured constant for the shape that does.
///
/// Measured for the shape it is used on — the engine's own lazy scan, end to
/// end, at each limit, and then the two candidates against each other with the
/// order alternating so that a machine drifting through the run cannot bias
/// the result — 65,536 rows was **10–12% faster** than 8,192. The sweep is
/// `crates/columnar-engine/examples/lazy_batch_size.rs`, and it uses
/// [`ParquetSource::batch_rows_under`] and
/// [`ParquetSource::read_with_batch_rows`] to change nothing but the limit.
///
/// The limit moves from 65,536 to **131,072** rows, because the limit is not
/// the batch size any more and a bigger limit finds a bigger divisor of the
/// row group's row count: at the old limit the published file's row group of a
/// million rows divided by 62,500, and at the new one it divides by 125,000.
///
/// The limit is **measured**, and it is measured over the two shapes the lazy
/// path serves, because the two pull in opposite directions: the scan with one
/// materialised column and one predicate is fastest at 125,000 rows, and the
/// real Q6's predicate set — two columns, one of them a dictionary-encoded
/// string — is fastest at 250,000. 131,072 is the limit that is best on one
/// shape and within 1% of the best on the other. See
/// [§3e](../../PERF.md#3e-the-scans-decomposition-and-what-the-compaction-was-worth).
///
/// The engine's own operator batch size is a separate thing, and unrelated.
pub const MAX_READER_BATCH_ROWS: usize = 131_072;

/// The smallest batch size the reader may use, in rows.
///
/// A row group whose row count has no usable divisor — a prime, say — is read
/// in batches of this size, and it ends with a partial batch. That is the cost
/// of an awkward row count, and it is paid once per row group.
pub(crate) const MIN_READER_BATCH_ROWS: usize = 4_096;

/// How the reader's batch size is chosen for a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchPlan {
    /// A fixed number of rows, whatever the row groups hold.
    ///
    /// This is the **measurement seam**: it can leave a partial batch at the
    /// end of every row group, which is what the probes measure and what the
    /// engine's own read does not do.
    Fixed(usize),
    /// The largest divisor of the row groups' row count under a limit.
    Derived(usize),
}

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
    /// How many rows each row group holds.
    ///
    /// The reader's batch size is a divisor of a row group's row count, so
    /// the read needs to know them.
    row_group_rows: Vec<usize>,
    /// The smallest and largest value each column holds, over the file's row
    /// groups.
    ///
    /// A column with no statistics in any row group is not in the map.
    ranges: ColumnRanges,
    /// The range of each column over each row group, with the row group's
    /// rows.
    ///
    /// A row group that has no statistics for a column has no range for it,
    /// and its rows cannot be bounded by it.
    row_group_ranges: RowGroupRanges,
}

/// A column's range over a whole file.
type ColumnRange = (ScalarValue, ScalarValue);

/// The range of every column over a file.
type ColumnRanges = HashMap<String, ColumnRange>;

/// The range of every column over every row group.
type RowGroupRanges = HashMap<String, Vec<RowGroupRange>>;

/// A column's range over one row group, and the row group's rows.
#[derive(Debug, Clone)]
pub struct RowGroupRange {
    /// The column's range in this row group, if the row group has statistics
    /// for it.
    pub range: Option<(ScalarValue, ScalarValue)>,
    /// The row group's rows.
    pub rows: usize,
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
        let row_group_rows: Vec<usize> = metadata
            .row_groups()
            .iter()
            .map(|group| usize::try_from(group.num_rows()).unwrap_or(0))
            .collect();

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

        let (ranges, row_group_ranges) = column_ranges(metadata);

        Ok(Self {
            path: path.to_path_buf(),
            schema,
            columns,
            rows,
            row_groups,
            row_group_rows,
            ranges,
            row_group_ranges,
        })
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

    /// The smallest and largest value a column holds, over the file's row groups.
    ///
    /// `None` when the column has no statistics, or when they are not of a
    /// type the range can be expressed in.
    #[must_use]
    pub fn column_range(&self, name: &str) -> Option<ColumnRange> {
        self.ranges.get(name).cloned()
    }

    /// The range of a column over each row group, with the row group's rows.
    ///
    /// A row group that has no statistics for the column has no range for it,
    /// and its rows cannot be bounded by it. A column with no statistics in
    /// any row group has no entry at all.
    ///
    /// This is what makes a **bound** rather than a guess: a row group whose
    /// own range cannot satisfy a predicate holds no row that can satisfy it,
    /// so its rows need not be counted.
    #[must_use]
    pub fn row_group_ranges(&self, name: &str) -> &[RowGroupRange] {
        self.row_group_ranges.get(name).map_or(&[], Vec::as_slice)
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
        self.read_with(
            columns,
            pushdown,
            flavor,
            threads,
            BatchPlan::Derived(MAX_READER_BATCH_ROWS),
        )
    }

    /// Reads the file with a given reader batch size.
    ///
    /// This is the **measurement seam**. The batch size the engine's own read
    /// uses is *derived from the file* -- see
    /// [`ParquetSource::batch_rows_under`] -- and this method overrides that
    /// derivation with an explicit number, so that a probe can measure one
    /// batch size against another.
    ///
    /// A batch size that does not divide a row group's row count leaves a
    /// **partial batch** at the end of every row group, and a partial batch
    /// costs the parquet crate's reader about 0.85 ms. That is what the seam
    /// exists to measure, and it is why the engine does not use it.
    pub fn read_with_batch_rows(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
        batch_rows: usize,
    ) -> Result<Dataset, DatasetError> {
        self.read_with(columns, pushdown, flavor, threads, BatchPlan::Fixed(batch_rows))
    }

    /// The reader's batch size under a given limit, for the whole file.
    ///
    /// The batch size is the **largest divisor of the row groups' row count**
    /// that fits under `limit`, so that no row group ends with a partial
    /// batch. A partial batch costs about 0.85 ms, once per row group, and
    /// the incumbents do not pay it.
    ///
    /// A parallel read derives the batch **per worker**, from the row groups
    /// that worker reads, because a row group is decoded independently of
    /// every other one. For a file whose row groups hold the same number of
    /// rows -- every file this repository's generator writes -- the two are
    /// the same number.
    #[must_use]
    pub fn batch_rows_under(&self, limit: usize) -> usize {
        self.batch_rows_for(None, limit)
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

    /// Reads the file, with the batch size the plan names.
    fn read_with(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
        plan: BatchPlan,
    ) -> Result<Dataset, DatasetError> {
        let workers = self.worker_count(threads);
        if workers == 1 {
            return self.read_row_groups(columns, pushdown, flavor, None, plan);
        }
        self.read_parallel(columns, pushdown, flavor, workers, self.row_groups, plan)
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
        plan: BatchPlan,
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
                        source.read_row_groups(columns, pushdown, flavor, Some(block.clone()), plan)
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

    /// The reader's batch size for a set of row groups.
    ///
    /// The batch size has to **divide** the row group's row count, or the row
    /// group ends with a partial batch -- and a partial batch is expensive:
    /// the parquet crate's reader pays about 0.85 ms for it, once per row
    /// group. That was the whole of the engine's per-row-group cost, and the
    /// incumbents do not pay it because their readers do not leave a partial
    /// batch behind. See [ADR-0023](../../../docs/adr/0023-the-per-row-group-cost.md).
    ///
    /// The read therefore uses the **greatest common divisor** of the row
    /// groups' row counts, which divides every one of them, rounded down to a
    /// divisor that fits under the limit. For a file with uniform row groups
    /// that is one batch a row group, and no partial batch anywhere.
    ///
    /// A divisor under the **floor** is not used: a batch of a handful of
    /// rows would cost more in per-batch overhead than it saves. When the row
    /// counts are awkward -- a prime, say -- the limit bounds the batch
    /// instead, the smallest row count bounds it too, and the row group ends
    /// with a partial batch. That is the cost of an awkward row count, and it
    /// is paid once per row group.
    fn batch_rows_for(&self, row_groups: Option<&[usize]>, limit: usize) -> usize {
        let counts: Vec<usize> = match row_groups {
            Some(groups) => {
                groups.iter().filter_map(|index| self.row_group_rows.get(*index).copied()).collect()
            }
            None => self.row_group_rows.clone(),
        };
        if counts.is_empty() {
            return limit;
        }

        let smallest = counts.iter().copied().min().unwrap_or(1).max(1);
        let divisor = counts.iter().copied().fold(0usize, gcd).max(1);
        let aligned = largest_divisor_at_most(divisor, limit);
        if aligned >= MIN_READER_BATCH_ROWS {
            return aligned;
        }

        // No usable divisor: the row counts are awkward — a prime, say. The
        // limit then bounds the batch and the smallest row count bounds it
        // too, and the row group ends with a partial batch. That is the cost
        // of an awkward row count, and it is paid once per row group.
        limit.min(smallest)
    }

    /// Reads the given row groups -- or all of them, when `row_groups` is
    /// `None`.
    fn read_row_groups(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        row_groups: Option<Vec<usize>>,
        plan: BatchPlan,
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

        let batch_rows = match plan {
            BatchPlan::Fixed(rows) => rows,
            BatchPlan::Derived(limit) => self.batch_rows_for(row_groups.as_deref(), limit),
        };
        let mut builder = builder.with_projection(mask).with_batch_size(batch_rows);
        if let Some(row_groups) = row_groups {
            builder = builder.with_row_groups(row_groups);
        }

        let reader = builder.build().map_err(|error| DatasetError::Parquet(error.to_string()))?;
        decode_batches_filtered(reader, &self.schema, columns, &projection, pushdown, flavor)
    }
}

/// The smallest and largest value each column holds, over the file's row groups.
///
/// A column with no statistics in any row group is not in the map at all: the
/// range is what the file's footer says, and a file that says nothing has no
/// range to report.
fn column_ranges(metadata: &ParquetMetaData) -> (ColumnRanges, RowGroupRanges) {
    let group_count = metadata.num_row_groups();
    let mut ranges: ColumnRanges = HashMap::new();
    // One entry per row group per column: `None` where the row group has no
    // statistics for the column, so that its rows cannot be bounded away.
    let mut per_group: HashMap<String, Vec<Option<ColumnRange>>> = HashMap::new();

    for (index, group) in metadata.row_groups().iter().enumerate() {
        for chunk in group.columns() {
            let name = chunk.column_path().string();
            let range = chunk.statistics().and_then(statistics_range);
            let column = per_group.entry(name.clone()).or_insert_with(|| vec![None; group_count]);
            column[index].clone_from(&range);
            let Some((min, max)) = range else {
                continue;
            };
            ranges
                .entry(name)
                .and_modify(|range| {
                    if scalar_cmp(&min, &range.0) == Ordering::Less {
                        range.0 = min.clone();
                    }
                    if scalar_cmp(&max, &range.1) == Ordering::Greater {
                        range.1 = max.clone();
                    }
                })
                .or_insert((min, max));
        }
    }

    let row_group_ranges = per_group
        .into_iter()
        .map(|(name, column)| {
            let groups = column
                .into_iter()
                .enumerate()
                .map(|(index, range)| RowGroupRange {
                    range,
                    rows: usize::try_from(metadata.row_groups()[index].num_rows()).unwrap_or(0),
                })
                .collect();
            (name, groups)
        })
        .collect();

    (ranges, row_group_ranges)
}

/// The smallest and largest value a column chunk's statistics hold.
///
/// `None` when the statistics are of a type the engine has no scalar for, or
/// when the chunk records no minimum or no maximum.
fn statistics_range(statistics: &Statistics) -> Option<(ScalarValue, ScalarValue)> {
    match statistics {
        Statistics::Boolean(_) | Statistics::Int96(_) | Statistics::FixedLenByteArray(_) => None,
        Statistics::Int32(values) => Some((
            ScalarValue::Int64(i64::from(*values.min_opt()?)),
            ScalarValue::Int64(i64::from(*values.max_opt()?)),
        )),
        Statistics::Int64(values) => {
            Some((ScalarValue::Int64(*values.min_opt()?), ScalarValue::Int64(*values.max_opt()?)))
        }
        Statistics::Float(values) => Some((
            ScalarValue::Float64(f64::from(*values.min_opt()?)),
            ScalarValue::Float64(f64::from(*values.max_opt()?)),
        )),
        Statistics::Double(values) => Some((
            ScalarValue::Float64(*values.min_opt()?),
            ScalarValue::Float64(*values.max_opt()?),
        )),
        Statistics::ByteArray(values) => {
            let min = std::str::from_utf8(values.min_opt()?.data()).ok()?;
            let max = std::str::from_utf8(values.max_opt()?.data()).ok()?;
            Some((ScalarValue::Utf8(min.to_owned()), ScalarValue::Utf8(max.to_owned())))
        }
    }
}

/// The order of two values of the same type.
///
/// Two values of different types cannot be compared, and the engine never
/// compares them: a column has one type, so its statistics have one type too.
fn scalar_cmp(left: &ScalarValue, right: &ScalarValue) -> Ordering {
    match (left, right) {
        (ScalarValue::Int64(left), ScalarValue::Int64(right)) => left.cmp(right),
        (ScalarValue::Float64(left), ScalarValue::Float64(right)) => left.total_cmp(right),
        (ScalarValue::Utf8(left), ScalarValue::Utf8(right)) => left.cmp(right),
        _ => Ordering::Equal,
    }
}

/// The greatest common divisor of two numbers.
pub(crate) fn gcd(left: usize, right: usize) -> usize {
    let mut a = left;
    let mut b = right;
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// The largest divisor of `rows` that is at most `limit`.
///
/// The divisors are found by trial division, which is `sqrt(rows)` steps, and
/// a row group's row count is a few hundred thousand at most.
pub(crate) fn largest_divisor_at_most(rows: usize, limit: usize) -> usize {
    if rows <= limit {
        return rows;
    }
    let mut best = 1usize;
    let mut divisor = 1usize;
    while divisor <= rows / divisor {
        if rows % divisor == 0 {
            if divisor <= limit {
                best = best.max(divisor);
            }
            let other = rows / divisor;
            if other <= limit {
                best = best.max(other);
            }
        }
        divisor += 1;
    }
    best
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

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::path::Path;

    use super::{
        gcd, largest_divisor_at_most, ParquetSource, MAX_READER_BATCH_ROWS, MIN_READER_BATCH_ROWS,
    };

    #[test]
    fn the_greatest_common_divisor_is_the_greatest_one() {
        assert_eq!(gcd(0, 7), 7);
        assert_eq!(gcd(7, 0), 7);
        assert_eq!(gcd(12, 18), 6);
        assert_eq!(gcd(18, 12), 6);
        assert_eq!(gcd(250_000, 1_000_000), 250_000);
        assert_eq!(gcd(1_000_000, 1_000_000), 1_000_000);
    }

    #[test]
    fn the_largest_divisor_under_the_limit_is_the_largest_one() {
        // A million rows: the largest divisor under the limit is 125,000.
        assert_eq!(largest_divisor_at_most(1_000_000, MAX_READER_BATCH_ROWS), 125_000);
        // A quarter of a million rows: the largest divisor under the limit.
        assert_eq!(largest_divisor_at_most(250_000, MAX_READER_BATCH_ROWS), 125_000);
        // Under the limit, the row count is its own largest divisor.
        assert_eq!(largest_divisor_at_most(1_024, MAX_READER_BATCH_ROWS), 1_024);
        // A prime has one divisor, and it is one.
        assert_eq!(largest_divisor_at_most(999_983, MAX_READER_BATCH_ROWS), 1);
    }

    #[test]
    fn the_largest_divisor_always_divides_the_row_count() {
        for rows in [1_000_000usize, 250_000, 500_000, 65_536, 131_072, 999_983] {
            let divisor = largest_divisor_at_most(rows, MAX_READER_BATCH_ROWS);
            assert_eq!(rows % divisor, 0, "{rows} is not divisible by {divisor}");
            assert!(divisor <= MAX_READER_BATCH_ROWS);
            assert!(divisor <= rows);
        }
    }

    #[test]
    fn the_batch_size_ends_a_row_group_without_a_partial_batch() {
        // The rule is the point of the whole exercise: the reader's batch
        // size has to divide a row group's row count, or the row group ends
        // with a partial batch, and the crate's reader pays for it.
        let rows = [1_000_000usize, 250_000, 500_000];
        let divisor = rows.iter().copied().fold(0usize, gcd).max(1);
        let batch = largest_divisor_at_most(divisor, MAX_READER_BATCH_ROWS);
        assert!(batch >= MIN_READER_BATCH_ROWS);
        for count in rows {
            assert_eq!(count % batch, 0, "a row group of {count} rows ends with a partial batch");
        }
    }

    #[test]
    fn the_batch_size_of_a_file_is_the_one_its_row_groups_agree_on() {
        let path = std::path::Path::new("/tmp/columnar-engine-batch-rows.parquet");
        write_row_groups(path, &[1_000, 1_000, 500]);
        let source = ParquetSource::open(path).expect("the source opens");

        // The greatest common divisor of a thousand and five hundred is five
        // hundred, and five hundred divides every row group.
        let batch = source.batch_rows_under(MAX_READER_BATCH_ROWS);
        assert_eq!(batch, 500);
        for count in [1_000usize, 500] {
            assert_eq!(count % batch, 0);
        }
    }

    #[test]
    fn a_limit_below_the_floor_takes_the_limit_and_says_so() {
        let path = std::path::Path::new("/tmp/columnar-engine-batch-floor.parquet");
        write_row_groups(path, &[1_000, 1_000, 500]);
        let source = ParquetSource::open(path).expect("the source opens");

        // Five hundred is under the floor, so the limit bounds the batch
        // instead -- and the row group ends with a partial batch, which is
        // the documented cost of an awkward row count.
        assert_eq!(source.batch_rows_under(64), 64);
        // Under the floor, the row count itself is the batch.
        assert_eq!(source.batch_rows_under(1_000_000), 500);
    }

    /// Writes a Parquet file with one row group per batch.
    fn write_row_groups(path: &Path, row_counts: &[usize]) {
        use arrow::array::Int64Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        use parquet::arrow::ArrowWriter;
        use parquet::file::properties::WriterProperties;
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, false)]));
        let file = File::create(path).expect("the file is created");
        // The writer's own row-group size decides where the row groups fall,
        // so it is set to the first batch's row count: the groups are then
        // the batches, and the last one holds the remainder.
        let properties = WriterProperties::builder()
            .set_max_row_group_size(row_counts.first().copied().unwrap_or(1))
            .build();
        let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(properties))
            .expect("the writer is built");
        for count in row_counts {
            let count = i64::try_from(*count).expect("the row count fits in an i64");
            let values: Vec<i64> = (0..count).collect();
            let batch =
                RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(values))])
                    .expect("the batch is built");
            writer.write(&batch).expect("the batch is written");
        }
        writer.close().expect("the file is closed");
    }
}
