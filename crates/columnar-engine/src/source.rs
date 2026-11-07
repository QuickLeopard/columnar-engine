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
use arrow::record_batch::RecordBatch;
use columnar_core::{DataType, ScalarValue};
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ProjectionMask;
use parquet::file::metadata::ParquetMetaData;
use parquet::file::statistics::Statistics;

use crate::dataset::{decode_batches_filtered, merge_scans, Dataset, DatasetError, ScanOutput};
use crate::plan::{ColumnMapping, ColumnSchema, ResolvedFilter};
use crate::pushdown::{Pushdown, PushdownPlan, RowMask};
use crate::EngineError;
use columnar_kernels::Flavor;

/// What a read takes: the columns the dataset holds, and the columns the
/// reader fetches.
///
/// **The reader's own decision**, made the way the engine makes it: the
/// plan's materialised columns are held for the whole read, a predicate the
/// reader **declined** has its column read and held too, and a predicate the
/// reader **pushed** has its column fetched and dropped.
///
/// The two sets are what the estimate charges: the **file's rows** for the
/// held ones, and the **batch's rows** for the fetched ones. See
/// [`crate::memory::estimate_lazy_read_with_batch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadShape {
    /// The file's root columns the dataset holds.
    pub held: Vec<usize>,
    /// The file's root columns the reader fetches.
    pub fetched: Vec<usize>,
}

/// What a scan hands each batch to, before it copies anything.
///
/// The scan copies a row **once**, into the engine's own column. A filter
/// that runs before the copy decides which rows are copied at all, and the
/// rows it drops are never copied.
///
/// The join is such a filter. The rows it does not match are worth nothing to
/// the query, and the scan that copies them copies 96 MB to keep 3.5 MB. See
/// [ADR-0030](../../../docs/adr/0030-the-scan-that-feeds-the-join.md).
pub trait BatchFilter: Sync {
    /// The rows of `batch` to keep, as the batch's own row positions.
    ///
    /// The positions are in **ascending order**, and a position may repeat:
    /// the join's output has one row per match, and a probe row that matches
    /// twice is two rows of the result.
    ///
    /// `survivors` are the rows the pushed-down predicates kept, if they kept
    /// any. The filter must keep only rows the mask keeps.
    fn keep(
        &self,
        batch: &RecordBatch,
        survivors: Option<&RowMask>,
    ) -> Result<Vec<u32>, DatasetError>;
}

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
    ///
    /// The field is **crate-private** on purpose: Arrow's type is the
    /// reader's business, and the engine's surface does not name Arrow's
    /// types. A reader who wants the engine's own type has `data_type`.
    pub(crate) arrow_type: ArrowType,
}

impl SourceColumn {
    /// How the column's values are stored.
    ///
    /// **The schema's own Arrow type is the signal**, and not the chunk's
    /// encodings: a chunk's `encodings` list names `RLE_DICTIONARY` for a
    /// column that is not dictionary-encoded at all. See [`crate::width`].
    #[must_use]
    pub fn storage(&self) -> crate::width::Storage {
        crate::width::storage_of(&self.arrow_type)
    }
}

/// The reader's default **byte budget**, in bytes.
///
/// **The default's rule is a budget and not a row count**, because the bytes a
/// batch holds scale with the query's width: the same million-row batch is
/// **12.8 MiB** on a two-column query and **165.8 MiB** on a sixteen-column one,
/// and a limit named in rows cannot see the width. See
/// [ADR-0061](../../../docs/adr/0061-the-batch-sizes-price.md).
///
/// **16 MiB** is bounded by the measurement from **both sides**: above the narrow
/// shapes' whole row group (**12.8 MiB**) and at or below the wide shape's own
/// batch (**20.7 MiB**). And the rule gives **every** shape at least what it had:
/// the narrow shapes get the whole row group, and the wide shape gets
/// **62,500** rows, which the measurement says is a small win.
///
/// **The budget is the read's, and the workers share it.** A parallel read
/// holds one batch per worker, so a read on six workers gives each of them a
/// sixth of the budget. **At one worker -- the measurement's setting, and the
/// headline's -- the rule is exactly the published one**: the narrow shapes get
/// the whole row group and the wide shape gets 62,500 rows.
///
/// **The batch size is still a divisor of the row group's row count**, so that
/// no row group ends with a partial batch -- a partial batch costs the parquet
/// crate's reader about 0.85 ms, once per row group, and the incumbents do not
/// pay it. See [ADR-0023](../../../docs/adr/0023-the-per-row-group-cost.md).
pub const DEFAULT_READER_BATCH_BYTES: usize = 16 * 1024 * 1024;

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
    /// The largest divisor of the row groups' row count whose bytes fit a
    /// budget.
    ///
    /// **This is the engine's own**, and the budget is divided by what the read
    /// fetches, so that two widths get two batches. **The budget is the read's,
    /// and the workers share it** -- the plan that reaches a worker carries the
    /// worker's share.
    UnderBytes { budget: usize },
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
    /// The reader's batch size, in rows, when the caller names one.
    ///
    /// **`None` is the engine's own**: the largest divisor of the row groups'
    /// row count whose bytes fit [`DEFAULT_READER_BATCH_BYTES`], so that no row
    /// group ends with a partial batch.
    ///
    /// **This is a knob, and it is not the default.** A named size is the
    /// measurement seam, and the published figures that name one stay
    /// reproducible by the command that made them.
    reader_batch_rows: Option<usize>,
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
    /// The **file's own** bytes, for each column, over the whole file.
    ///
    /// **The file's pages**: the column chunk's `total_uncompressed_size`,
    /// summed over the row groups. It is not the working set's cost — the
    /// engine holds decoded values — so it is used for the one thing it is
    /// good at: the size of a column's own data. See [`crate::width`].
    file_bytes: Vec<usize>,
    /// The values each column holds, over the whole file.
    ///
    /// The file's own accounting of how many values a column has, so the
    /// file's own bytes can be said **a row**.
    file_values: Vec<usize>,
}

/// A column's range over a whole file.
pub(crate) type ColumnRange = (ScalarValue, ScalarValue);

/// The range of every column over a file.
pub(crate) type ColumnRanges = HashMap<String, ColumnRange>;

/// The range of every column over every row group.
pub(crate) type RowGroupRanges = HashMap<String, Vec<RowGroupRange>>;

/// A column's range over one row group, and the row group's rows.
#[derive(Debug, Clone)]
pub struct RowGroupRange {
    /// The column's range in this row group, if the row group has statistics
    /// for it.
    pub range: Option<(ScalarValue, ScalarValue)>,
    /// The row group's rows.
    pub rows: usize,
}

/// What the engine needs of a source, so that one query path serves a file and
/// a set of files.
///
/// The trait is the engine's own **seam**, and it is crate-private: the
/// surface carries the two sources themselves -- [`ParquetSource`] and
/// [`crate::file_set::FileSetSource`] -- and the path that reads them. The
/// trait is how that one path is written once.
///
/// **The trait is not a second set of rules.** Every method is the source's
/// own method, and the file set's are the file's own rules applied to a set:
/// the same decision, the same reader, the same batch rule.
pub(crate) trait EngineSource: ColumnSchema {
    /// How many rows the source holds.
    fn rows(&self) -> usize;

    /// The source's columns, in schema order.
    fn columns(&self) -> &[SourceColumn];

    /// The index of a named column, if the source has one.
    ///
    /// The build side looks its key up by name, and the estimate charges the
    /// key's range: both are the same question, asked of any source.
    fn column_index(&self, name: &str) -> Option<usize>;

    /// The range of a named column over the whole source.
    ///
    /// A file's range is the file's own. A set's range is the **union** of the
    /// files' ranges -- the range the set's rows actually span, which is what
    /// a direct-addressed table over that key is sized by.
    fn column_range(&self, name: &str) -> Option<ColumnRange>;

    /// The range of a named column over each of the source's row groups.
    ///
    /// The estimate's bound on the rows a filter lets through is made over
    /// these, and a set's are the files' own, in the files' order.
    fn row_group_ranges(&self, name: &str) -> &[RowGroupRange];

    /// The names of the columns at `indices`, in the order given.
    fn column_names(&self, indices: &[usize]) -> Vec<String>;

    /// How many workers a read of this source uses.
    fn worker_count(&self, threads: usize) -> usize;

    /// The reader's batch size a read of these columns will use, in rows, when
    /// the read runs on `workers` workers.
    ///
    /// **The engine's own**: the largest divisor of the row groups' row count
    /// whose bytes fit the default's budget — or the size the caller named.
    /// **The workers share the budget**, so the read's batch memory never
    /// exceeds it.
    ///
    /// **The estimate reads it**, so the budget can see what a read will hold
    /// before the read holds it.
    fn batch_rows_for(&self, read: &[usize], workers: usize) -> usize;

    /// Decides what the read takes, and what it pushes into the reader.
    ///
    /// The rules are [`ParquetSource::decide_read`]'s, and they are the same
    /// rules for every source: the plan's materialised set, and the plan's
    /// predicates.
    fn decide_read(
        &self,
        materialised: &[usize],
        filters: &[ResolvedFilter],
    ) -> Result<ReadDecision, EngineError>;

    /// Reads the given columns, with the given predicates pushed down.
    fn read(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
    ) -> Result<Dataset, DatasetError>;

    /// Reads the given columns, with a filter, and the filter's rows.
    fn read_filtered(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: &(dyn BatchFilter + Sync),
        flavor: Flavor,
        threads: usize,
    ) -> Result<ScanOutput, DatasetError>;
}

/// What a read of a file takes, and what it pushes into the reader.
///
/// The decision is made **once**, for every read in the engine: the probe
/// side's scan and the join's build side ask the same question, and the
/// reader's rules are the same rules for both.
///
/// The rules, in order:
///
/// 1. The columns the caller materialises are read.
/// 2. A predicate whose column is **already being read** is declined. The
///    engine's filter stage applies the predicates the reader did not take,
///    over the rows that survived the read. Pushing this one into the reader
///    would evaluate it over every row of the file *and* leave the filter to
///    evaluate it again over the survivors; declining it leaves the work to
///    the filter, once, over fewer rows.
/// 3. A predicate the pushdown cannot evaluate **exactly** is declined. It
///    is never approximated.
/// 4. A declined predicate's column is read, because the engine's filter
///    stage will need it.
#[derive(Debug)]
pub(crate) struct ReadDecision {
    /// The file's root indices to read, ascending.
    pub(crate) read: Vec<usize>,
    /// The predicates to push into the reader.
    pub(crate) pushdown: PushdownPlan,
    /// The predicates the engine's own filter has to apply.
    ///
    /// A predicate the reader did not take is applied by the engine, over the
    /// rows that survived the read -- **once**, and by one evaluator.
    pub(crate) declined: Vec<ResolvedFilter>,
    /// Whether **every** predicate was pushed into the reader.
    ///
    /// A plan whose predicates were all pushed has no filter work left, so the
    /// read decides the rows on its own.
    pub(crate) fully_pushed: bool,
}

impl ReadDecision {
    /// The mapping from the file's columns onto the read set.
    ///
    /// The read set holds every column the caller materialises, and the
    /// declined predicates' columns beside them, so the mapping holds every
    /// column the plan names.
    pub(crate) fn mapping(&self, columns: usize) -> ColumnMapping {
        ColumnMapping::of(&self.read, columns)
    }
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
        let (file_bytes, file_values) = file_bytes(metadata);

        Ok(Self {
            path: path.to_path_buf(),
            schema,
            columns,
            rows,
            row_groups,
            row_group_rows,
            reader_batch_rows: None,
            ranges,
            row_group_ranges,
            file_bytes,
            file_values,
        })
    }

    /// The reader's batch size, in rows, when the caller names one.
    ///
    /// **The knob.** The reader hands the rows over in batches, and the batch
    /// size is a **knob**: the engine's own is the largest divisor of the row
    /// groups' row count whose bytes fit [`DEFAULT_READER_BATCH_BYTES`], so
    /// that no row group ends with a partial batch.
    ///
    /// **A named size is exactly that size**, and a size that does not divide
    /// a row group's row count leaves a **partial batch** at the end of every
    /// row group. A partial batch costs the parquet crate's reader about
    /// **0.85 ms**, once per row group, and the engine's own read does not
    /// pay it -- a knob that names a size does.
    ///
    /// **A named size is not the default.** Nothing named is the engine's own,
    /// so every published figure that names one stays reproducible by the
    /// command that made it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// use columnar_engine::ParquetSource;
    ///
    /// // The file is the repository's own, so the example is not run.
    /// let source = ParquetSource::open(Path::new("data/lineitem.parquet"))?
    ///     .with_reader_batch_rows(250_000);
    /// # Ok::<(), columnar_engine::DatasetError>(())
    /// ```
    #[must_use]
    pub fn with_reader_batch_rows(mut self, rows: usize) -> Self {
        self.reader_batch_rows = Some(rows);
        self
    }

    /// The batch plan this source reads with.
    ///
    /// **The knob's plan**: the engine's own derived batch size, or the size
    /// the caller named.
    fn batch_plan(&self) -> BatchPlan {
        self.reader_batch_rows.map_or(
            BatchPlan::UnderBytes {
                budget: DEFAULT_READER_BATCH_BYTES,
            },
            BatchPlan::Fixed,
        )
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

    /// How many rows each of the file's row groups holds.
    ///
    /// This is the crate's own seam: the file set merges the row groups of
    /// its files into one numbering, and it needs their rows to do it.
    #[must_use]
    pub(crate) fn row_group_row_counts(&self) -> &[usize] {
        &self.row_group_rows
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

    /// How a column's values are stored.
    ///
    /// **The schema's own Arrow type is the signal**, and not the chunk's
    /// encodings: a chunk's `encodings` list names `RLE_DICTIONARY` for a
    /// column that is not dictionary-encoded at all. See [`crate::width`].
    #[must_use]
    pub fn column_storage(&self, index: usize) -> Option<crate::width::Storage> {
        self.columns.get(index).map(SourceColumn::storage)
    }

    /// The value's own bytes a row, of the column at `index`.
    ///
    /// **The file's own accounting** of the column, and **nothing for a
    /// number**: a number's eight bytes already are its value, so the file's
    /// pages for it would be charged twice.
    #[must_use]
    pub fn column_value_bytes(&self, index: usize) -> usize {
        match self.columns.get(index).map(SourceColumn::storage) {
            Some(crate::width::Storage::Fixed64) | None => 0,
            Some(_) => self.file_bytes.get(index).copied().unwrap_or(0),
        }
    }

    /// The **file's own** values, of the column at `index`.
    ///
    /// The file's own account of how many values the column holds, so the
    /// file's own bytes can be said **a row**.
    #[must_use]
    pub fn column_file_values(&self, index: usize) -> usize {
        self.file_values.get(index).copied().unwrap_or(0)
    }

    /// The **file's own** bytes a row, of the column at `index`.
    ///
    /// **The file's pages**: the column chunk's `total_uncompressed_size`
    /// over its `num_values`. It is **not** the working set's cost -- the
    /// engine holds decoded values, and the file stores `l_discount` in
    /// **0.50** bytes a row where the engine holds it in **eight** -- and it
    /// is published beside the statement so that the difference is visible.
    /// See [`crate::width`].
    #[must_use]
    pub fn column_file_bytes(&self, index: usize) -> usize {
        self.file_bytes.get(index).copied().unwrap_or(0)
    }

    /// What a read of this file takes: the columns the dataset holds, and the
    /// columns the reader fetches.
    ///
    /// **The reader's own decision**, made the way the engine makes it, so
    /// that a caller cannot answer a different question from the one the
    /// estimate answers. See [`ReadShape`].
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownColumn`] when a predicate names a column
    /// the file does not have.
    pub fn read_shape(
        &self,
        materialised: &[usize],
        filters: &[ResolvedFilter],
    ) -> Result<ReadShape, EngineError> {
        let decision = self.decide_read(materialised, filters)?;
        let fetched = crate::scan::fetched_columns(&decision);
        Ok(ReadShape { held: decision.read, fetched })
    }

    /// The bytes one row of the column at `index` costs, in the engine's own
    /// storage.
    ///
    /// **This is the width statement**, and the estimate and the reader's batch
    /// rule both read it. See [`crate::width`].
    #[must_use]
    pub fn column_cost(&self, index: usize) -> crate::width::RowCost {
        let storage = self
            .columns
            .get(index)
            .map_or(crate::width::Storage::StringHeader, SourceColumn::storage);
        storage.cost(self.column_value_bytes(index))
    }

    /// The reader's batch size a read of these columns will use, in rows, when
    /// the read runs on `workers` workers.
    ///
    /// **The engine's own**: the largest divisor of the row groups' row count
    /// whose **bytes** fit the default's budget — and the bytes a row costs are
    /// the **width statement's**, over the columns the read fetches. So two
    /// widths get two batches, and neither of them ends a row group with a
    /// partial batch.
    ///
    /// **The budget is the read's, and the workers share it**: a read on six
    /// workers gives each of them a sixth of it, so the read's batch memory
    /// never exceeds the budget.
    ///
    /// **The caller may name a size in rows instead**, and that is the
    /// measurement seam: [`ParquetSource::with_reader_batch_rows`].
    #[must_use]
    pub fn batch_rows_for(&self, read: &[usize], workers: usize) -> usize {
        match self.batch_plan() {
            BatchPlan::Fixed(rows) => rows,
            BatchPlan::UnderBytes { budget } => {
                self.batch_rows_under_bytes(budget, read, workers)
            }
        }
    }

    /// The reader's batch size under a **byte budget**, for a read of `read`.
    ///
    /// **The budget is in bytes, because the bytes a batch holds scale with the
    /// query's width**: the same million-row batch is a few megabytes on a
    /// two-column query and hundreds on a sixteen-column one, and a knob named
    /// in rows cannot see the width. See
    /// [ADR-0061](../../../docs/adr/0061-the-batch-sizes-price.md).
    ///
    /// The batch size is **the largest divisor of the row groups' row count**
    /// whose bytes fit under the budget — so no row group ends with a partial
    /// batch, and the budget is what the batch is measured against.
    ///
    /// **The width is the statement's**, the same number the estimate charges
    /// a column, so the rule and the estimate cannot disagree about what a row
    /// costs.
    #[must_use]
    pub fn batch_rows_under_bytes(
        &self,
        budget_bytes: usize,
        read: &[usize],
        workers: usize,
    ) -> usize {
        self.batch_rows_under_bytes_for(None, budget_bytes, read, workers)
    }

    /// The budget's rule, over a set of row groups.
    ///
    /// A parallel read derives the batch **per worker**, from the row groups
    /// that worker reads, because a row group is decoded independently of every
    /// other one — and **the workers share the budget**, so the batch memory
    /// the read holds is the budget and not the budget times the workers.
    fn batch_rows_under_bytes_for(
        &self,
        row_groups: Option<&[usize]>,
        budget_bytes: usize,
        read: &[usize],
        workers: usize,
    ) -> usize {
        let per_row = self.row_bytes_of(read);
        if per_row == 0 {
            // A read that fetches nothing holds nothing, so the budget does not
            // bind it: the batch is the row group's own rows.
            return self.batch_rows_for_groups(row_groups, usize::MAX);
        }
        // **The floor**: a budget too small to buy the smallest usable batch
        // still buys that batch, because a read has to read something.
        let share = budget_bytes / workers.max(1);
        let limit = (share / per_row).max(MIN_READER_BATCH_ROWS);
        self.batch_rows_for_groups(row_groups, limit)
    }

    /// The bytes one row of a read of these columns costs.
    ///
    /// **The statement's per-column cost**, over the columns the read takes,
    /// and the validity bit each column's rows carry.
    #[must_use]
    pub fn row_bytes_of(&self, read: &[usize]) -> usize {
        read.iter()
            .filter_map(|&index| self.columns.get(index).map(|column| (index, column)))
            .map(|(index, _column)| {
                let cost = self.column_cost(index);
                cost.per_row(self.rows()).saturating_add(1)
            })
            .sum()
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
        self.read_with(columns, pushdown, flavor, threads, self.batch_plan())
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
        self.batch_rows_for_groups(None, limit)
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

    /// Decides what a read of this file takes, and what it pushes into the reader.
    ///
    /// The rules are the ones [`ReadDecision`] states, and they are the same
    /// rules for every read in the engine: the probe side's scan asks this
    /// question with the plan's materialised set and the plan's predicates,
    /// and the join's build side asks it with its own key as its materialised
    /// set and its own predicates.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownColumn`] when a predicate names a column
    /// the file does not have. A predicate the file cannot hold is a plan
    /// error, and it is reported as one rather than dropped.
    pub(crate) fn decide_read(
        &self,
        materialised: &[usize],
        filters: &[ResolvedFilter],
    ) -> Result<ReadDecision, EngineError> {
        decide_read(&self.columns, materialised, filters)
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
        let output = self.read_scanned(columns, pushdown, None, flavor, threads, plan)?;
        Ok(output.dataset)
    }

    /// Reads the file, handing every batch to `filter` before it copies.
    ///
    /// The filter decides which rows are copied into the dataset, and the rows
    /// it drops are never copied at all. The returned rows are the dataset's
    /// own row numbers, one per output row: the join's output, in the probe
    /// side's order.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Io`] if the file cannot be read, and
    /// [`DatasetError::Parquet`] if it is not one the reader understands, or
    /// if the filter breaks its contract.
    pub fn read_filtered(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: &(dyn BatchFilter + Sync),
        flavor: Flavor,
        threads: usize,
    ) -> Result<ScanOutput, DatasetError> {
        self.read_scanned(columns, pushdown, Some(filter), flavor, threads, self.batch_plan())
    }

    /// Reads the file, with or without a filter.
    fn read_scanned(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        threads: usize,
        plan: BatchPlan,
    ) -> Result<ScanOutput, DatasetError> {
        let workers = self.worker_count(threads);
        if workers == 1 {
            return self.read_row_groups(columns, pushdown, filter, flavor, None, plan, workers);
        }
        self.read_parallel(columns, pushdown, filter, flavor, workers, self.row_groups, plan)
    }

    /// Reads the file with several threads.
    ///
    /// The row groups are split into contiguous blocks, one per thread, and
    /// the parts are merged **in block order**. Contiguous blocks are what
    /// makes the merge order the file's order without any sorting.
    #[allow(clippy::too_many_arguments)]
    fn read_parallel(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        workers: usize,
        row_groups: usize,
        plan: BatchPlan,
    ) -> Result<ScanOutput, DatasetError> {
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
        let parts = std::thread::scope(|scope| -> Result<Vec<ScanOutput>, DatasetError> {
            let handles: Vec<_> = blocks
                .iter()
                .map(|block| {
                    let source = self;
                    scope.spawn(move || {
                        source.read_row_groups(
                            columns,
                            pushdown,
                            filter,
                            flavor,
                            Some(block.clone()),
                            plan,
                            workers,
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
        merge_scans(parts)
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
    fn batch_rows_for_groups(&self, row_groups: Option<&[usize]>, limit: usize) -> usize {
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

    /// The file's schema.
    ///
    /// This is the crate's own seam: a set of files reads them as one table,
    /// and the table's schema is the files' schema.
    #[must_use]
    pub(crate) fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Builds the reader over the given row groups -- or all of them, when
    /// `row_groups` is `None`.
    ///
    /// The reader is **the engine's own**: the same projection, the same
    /// batch size, the same pushdown. A set of files chains its files'
    /// readers and decodes them in one pass, so that the rows are copied
    /// **once**, into the table's own columns -- and the copy a merge would
    /// make is not made.
    pub(crate) fn reader(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        row_groups: Option<Vec<usize>>,
        batch_rows: usize,
    ) -> Result<ParquetRecordBatchReader, DatasetError> {
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
        builder.build().map_err(|error| DatasetError::Parquet(error.to_string()))
    }

    /// Reads the given row groups -- or all of them, when `row_groups` is
    /// `None`.
    #[allow(clippy::too_many_arguments)]
    fn read_row_groups(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        row_groups: Option<Vec<usize>>,
        plan: BatchPlan,
        workers: usize,
    ) -> Result<ScanOutput, DatasetError> {
        let batch_rows = match plan {
            BatchPlan::Fixed(rows) => rows,
            BatchPlan::UnderBytes { budget } => {
                // The budget is divided by **the columns this read fetches**,
                // so the batch is the read's own.
                let projection = scan_projection(columns, pushdown);
                self.batch_rows_under_bytes_for(
                    row_groups.as_deref(),
                    budget,
                    &projection,
                    workers,
                )
            }
        };
        let reader = self.reader(columns, pushdown, row_groups, batch_rows)?;
        let projection = scan_projection(columns, pushdown);
        decode_batches_filtered(
            reader,
            &self.schema,
            columns,
            &projection,
            pushdown,
            flavor,
            filter,
        )
    }
}

/// The **file's own** bytes and values, for each column of the file.
///
/// **The file's pages**: each column chunk's `total_uncompressed_size` and
/// its `num_values`, summed over the row groups. It is not the working set's
/// cost — the engine holds decoded values, and a number's eight bytes
/// already are its value — so it is used for the one thing it is good at:
/// the size of a column's own data. See [`crate::width`].
fn file_bytes(metadata: &ParquetMetaData) -> (Vec<usize>, Vec<usize>) {
    let mut totals: Vec<usize> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for group in metadata.row_groups() {
        for (index, chunk) in group.columns().iter().enumerate() {
            if totals.len() <= index {
                totals.resize(index + 1, 0);
                counts.resize(index + 1, 0);
            }
            totals[index] = totals[index]
                .saturating_add(usize::try_from(chunk.uncompressed_size()).unwrap_or(0));
            counts[index] =
                counts[index].saturating_add(usize::try_from(chunk.num_values()).unwrap_or(0));
        }
    }
    (totals, counts)
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
pub(crate) fn scalar_cmp(left: &ScalarValue, right: &ScalarValue) -> Ordering {
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
pub(crate) fn scan_projection(columns: &[usize], pushdown: &PushdownPlan) -> Vec<usize> {
    let mut projection = columns.to_vec();
    projection.extend(pushdown.columns());
    projection.sort_unstable();
    projection.dedup();
    projection
}

/// Decides what a read takes, and what it pushes into the reader.
///
/// The rules are [`ReadDecision`]'s, and they are the same rules for **every**
/// source: the plan's materialised columns are read, a predicate over a column
/// that is already being read is declined, a predicate the pushdown cannot
/// evaluate exactly is declined, and a declined predicate's column is read.
///
/// The rules live here, once, and every source asks this question through
/// them: two copies of these rules could disagree, and the disagreement would
/// show up as a plan that reads a different set of columns on each side of a
/// join.
///
/// # Errors
///
/// Returns [`EngineError::UnknownColumn`] when a predicate names a column the
/// source does not have.
pub(crate) fn decide_read(
    columns: &[SourceColumn],
    materialised: &[usize],
    filters: &[ResolvedFilter],
) -> Result<ReadDecision, EngineError> {
    let mut read = materialised.to_vec();
    let mut pushdown = PushdownPlan::new();
    let mut declined: Vec<ResolvedFilter> = Vec::new();
    let mut fully_pushed = true;

    for filter in filters {
        let Some(column) = columns.get(filter.column) else {
            return Err(EngineError::UnknownColumn(format!(
                "the plan reads column {}, and the file does not hold it",
                filter.column
            )));
        };
        if read.contains(&filter.column) {
            fully_pushed = false;
            declined.push(filter.clone());
            continue;
        }
        if !crate::pushdown::supports(&column.arrow_type) {
            fully_pushed = false;
            declined.push(filter.clone());
            continue;
        }
        // Two predicates on the same column are one conjunction over that
        // column, and the reader would decode the column once per
        // predicate. They are pushed together.
        match pushdown.for_column_mut(filter.column) {
            Some(existing) => {
                existing.and(filter.predicate.op, filter.predicate.value.clone());
            }
            None => {
                pushdown.push(Pushdown::new(
                    filter.column,
                    &column.name,
                    filter.predicate.op,
                    filter.predicate.value.clone(),
                ));
            }
        }
    }

    for filter in &declined {
        if !read.contains(&filter.column) {
            read.push(filter.column);
        }
    }

    read.sort_unstable();
    read.dedup();

    Ok(ReadDecision { read, pushdown, declined, fully_pushed })
}

impl EngineSource for ParquetSource {
    fn rows(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> &[SourceColumn] {
        &self.columns
    }

    fn column_index(&self, name: &str) -> Option<usize> {
        ParquetSource::column_index(self, name)
    }

    fn column_range(&self, name: &str) -> Option<ColumnRange> {
        ParquetSource::column_range(self, name)
    }

    fn row_group_ranges(&self, name: &str) -> &[RowGroupRange] {
        ParquetSource::row_group_ranges(self, name)
    }

    fn column_names(&self, indices: &[usize]) -> Vec<String> {
        ParquetSource::column_names(self, indices)
    }

    fn worker_count(&self, threads: usize) -> usize {
        ParquetSource::worker_count(self, threads)
    }

    fn batch_rows_for(&self, read: &[usize], workers: usize) -> usize {
        ParquetSource::batch_rows_for(self, read, workers)
    }

    fn decide_read(
        &self,
        materialised: &[usize],
        filters: &[ResolvedFilter],
    ) -> Result<ReadDecision, EngineError> {
        self.decide_read(materialised, filters)
    }

    fn read(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
    ) -> Result<Dataset, DatasetError> {
        ParquetSource::read(self, columns, pushdown, flavor, threads)
    }

    fn read_filtered(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: &(dyn BatchFilter + Sync),
        flavor: Flavor,
        threads: usize,
    ) -> Result<ScanOutput, DatasetError> {
        ParquetSource::read_filtered(self, columns, pushdown, filter, flavor, threads)
    }
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

    fn column_cost(&self, index: usize) -> crate::width::RowCost {
        ParquetSource::column_cost(self, index)
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

    use columnar_core::{ComparisonOp, Predicate};

    use super::{
        gcd, largest_divisor_at_most, ParquetSource, MIN_READER_BATCH_ROWS,
    };
    use crate::plan::ResolvedFilter;

    /// A predicate over a column, by the column's index.
    fn filter(column: usize, value: i64) -> ResolvedFilter {
        ResolvedFilter {
            column,
            predicate: Predicate::new(format!("column-{column}"), ComparisonOp::Gt, value),
        }
    }

    #[test]
    fn a_predicate_the_reader_can_take_is_pushed_and_not_declined() {
        let path = std::path::Path::new("/tmp/columnar-engine-decide-pushed.parquet");
        write_three_columns(path);
        let source = ParquetSource::open(path).expect("the source opens");

        // The key is the only column being read, and the predicate is over
        // the amount -- so the reader can take it.
        let decision = source.decide_read(&[0], &[filter(1, 5)]).expect("the decision is made");
        assert_eq!(decision.pushdown.len(), 1);
        assert!(decision.declined.is_empty());
        assert!(decision.fully_pushed);
        // The predicate's column is read by the reader, and it is not in the
        // dataset: the reader drops it afterwards.
        assert_eq!(decision.read, vec![0]);
    }

    #[test]
    fn a_predicate_on_a_column_already_being_read_is_declined() {
        let path = std::path::Path::new("/tmp/columnar-engine-decide-declined.parquet");
        write_three_columns(path);
        let source = ParquetSource::open(path).expect("the source opens");

        // The predicate is over the key, which is being read: the engine's
        // filter applies it over the rows that survived, and the reader is
        // not asked to evaluate it over every row of the file.
        let decision = source.decide_read(&[0], &[filter(0, 5)]).expect("the decision is made");
        assert_eq!(decision.pushdown.len(), 0);
        assert_eq!(decision.declined.len(), 1);
        assert!(!decision.fully_pushed);
        // The column is in the read set, because the filter will need it.
        assert_eq!(decision.read, vec![0]);
    }

    #[test]
    fn a_predicate_the_file_does_not_have_is_refused() {
        let path = std::path::Path::new("/tmp/columnar-engine-decide-refused.parquet");
        write_three_columns(path);
        let source = ParquetSource::open(path).expect("the source opens");

        // A predicate over a column the file does not have is a plan error,
        // and it is reported as one: a predicate nobody applies would
        // silently change the answer.
        let missing = source.decide_read(&[0], &[filter(9, 5)]);
        assert!(missing.is_err(), "a predicate over a column the file does not have is refused");
    }

    #[test]
    fn two_predicates_on_one_column_are_one_pushdown() {
        let path = std::path::Path::new("/tmp/columnar-engine-decide-conjunction.parquet");
        write_three_columns(path);
        let source = ParquetSource::open(path).expect("the source opens");

        let decision =
            source.decide_read(&[0], &[filter(1, 5), filter(1, 7)]).expect("the decision is made");
        assert_eq!(decision.pushdown.len(), 1);
        assert_eq!(decision.pushdown.predicates()[0].comparisons.len(), 2);
        assert!(decision.declined.is_empty());
    }

    #[test]
    fn the_scan_and_the_reader_ask_the_same_question() {
        let path = std::path::Path::new("/tmp/columnar-engine-decide-same.parquet");
        write_three_columns(path);
        let source = ParquetSource::open(path).expect("the source opens");

        let filters = vec![filter(1, 5), filter(0, 7)];
        let materialised = vec![0_usize];

        // The scan's decision and the reader's decision are the same
        // question, so they are the same answer: a second copy of the rules
        // is a second answer waiting to disagree with the first.
        let scan = crate::scan::decide_scan(
            &crate::plan::ResolvedPlan {
                filter: filters.clone(),
                projection: Vec::new(),
                aggregate: None,
                join: None,
                sort: None,
                materialised: materialised.clone(),
            },
            &source,
        )
        .expect("the decision is made");
        let reader = source.decide_read(&materialised, &filters).expect("the decision is made");

        assert_eq!(scan.read, reader.read);
        assert_eq!(scan.pushdown, reader.pushdown);
        assert_eq!(scan.declined, reader.declined);
        assert_eq!(scan.fully_pushed, reader.fully_pushed);
    }

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
        assert_eq!(largest_divisor_at_most(1_000_000, 131_072), 125_000);
        // A quarter of a million rows: the largest divisor under the limit.
        assert_eq!(largest_divisor_at_most(250_000, 131_072), 125_000);
        // Under the limit, the row count is its own largest divisor.
        assert_eq!(largest_divisor_at_most(1_024, 131_072), 1_024);
        // A prime has one divisor, and it is one.
        assert_eq!(largest_divisor_at_most(999_983, 131_072), 1);
    }

    #[test]
    fn the_largest_divisor_always_divides_the_row_count() {
        for rows in [1_000_000usize, 250_000, 500_000, 65_536, 131_072, 999_983] {
            let divisor = largest_divisor_at_most(rows, 131_072);
            assert_eq!(rows % divisor, 0, "{rows} is not divisible by {divisor}");
            assert!(divisor <= 131_072);
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
        let batch = largest_divisor_at_most(divisor, 131_072);
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
        let batch = source.batch_rows_under(131_072);
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

    /// A filter that records the size of every batch it is handed.
    #[derive(Default)]
    struct BatchSizes {
        /// The size of every batch, in the order the reader handed them over.
        sizes: std::sync::Mutex<Vec<usize>>,
    }

    impl crate::source::BatchFilter for BatchSizes {
        fn keep(
            &self,
            batch: &arrow::record_batch::RecordBatch,
            _survivors: Option<&crate::pushdown::RowMask>,
        ) -> Result<Vec<u32>, crate::DatasetError> {
            self.sizes.lock().expect("the lock is not poisoned").push(batch.num_rows());
            Ok((0..u32::try_from(batch.num_rows()).unwrap_or(0)).collect())
        }
    }

    /// The knob names the batch size, and the answer does not move.
    #[test]
    fn the_knob_names_the_batch_size_and_the_answer_does_not_move() {
        let path = std::path::Path::new("/tmp/columnar-engine-reader-batch-rows.parquet");
        write_row_groups(path, &[1_000, 1_000]);
        let source = ParquetSource::open(path).expect("the source opens");
        let plan = crate::pushdown::PushdownPlan::default();

        // The engine's own: the largest divisor of the row groups' row count.
        let engine = BatchSizes::default();
        let engine_rows = source
            .read_filtered(&[0], &plan, &engine, columnar_kernels::best(), 1)
            .expect("the file reads");
        let engine_sizes = engine.sizes.lock().expect("the lock is not poisoned").clone();
        assert_eq!(engine_sizes, vec![1_000, 1_000], "the engine's own batch size");

        // The knob's: the size the caller names, in rows.
        let named = BatchSizes::default();
        let named_rows = source
            .with_reader_batch_rows(100)
            .read_filtered(&[0], &plan, &named, columnar_kernels::best(), 1)
            .expect("the file reads");
        let named_sizes = named.sizes.lock().expect("the lock is not poisoned").clone();
        assert_eq!(
            named_sizes,
            vec![100; 20],
            "the knob's batch size, in rows: twenty batches of a hundred"
        );

        // **And the answer does not move.** The knob changes when the rows
        // arrive, not which rows arrive.
        assert_eq!(
            engine_rows.rows.len(),
            named_rows.rows.len(),
            "the same number of rows came back"
        );
        assert_eq!(engine_rows.rows, named_rows.rows, "and they are the same rows");
        assert_eq!(
            engine_rows.dataset.len(),
            named_rows.dataset.len(),
            "and the dataset holds the same rows"
        );
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
            .set_max_row_group_row_count(Some(row_counts.first().copied().unwrap_or(1)))
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

    /// Writes a Parquet file with three columns: two `int64` and one `utf8`.
    fn write_three_columns(path: &Path) {
        use arrow::array::{Int64Array, StringArray};
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        use parquet::arrow::ArrowWriter;
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Int64, false),
            Field::new("amount", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let file = File::create(path).expect("the file is created");
        let mut writer =
            ArrowWriter::try_new(file, schema.clone(), None).expect("the writer is built");
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1_i64, 2, 3])),
                Arc::new(Int64Array::from(vec![10_i64, 20, 30])),
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
            ],
        )
        .expect("the batch is built");
        writer.write(&batch).expect("the batch is written");
        writer.close().expect("the file is closed");
    }
}
