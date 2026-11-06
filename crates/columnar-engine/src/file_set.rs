//! A set of Parquet files the engine reads as **one table**.
//!
//! # Why this exists
//!
//! The store writes a table as a **set of files**: a commit appends one and
//! never modifies an old one, and the manifest names them
//! ([ADR-0044](../../../docs/adr/0044-the-stores-half.md)). **And the engine's
//! only source is one file** -- [`ParquetSource`] opens one Parquet file -- so
//! a store's table cannot be read by the engine today.
//!
//! The embedder's way around that is to open each file and run the query over
//! each one, and it is **wrong three times over**: the rows of a filtered
//! table are split across the files, the groups of an aggregate are split
//! across them, and the sides of a join are split across them.
//!
//! **And the boundary is free.** Spec 039 measured it: the published file's
//! own six row groups, read as one file and as six, gave 66.003 ms and
//! 64.114 ms, and the difference is -1.152 ms -- negative, and inside the one
//! file's own spread
//! ([ADR-0045](../../../docs/adr/0045-the-file-boundarys-cost.md)). **So this
//! source's case is correctness, not performance**, and it has to be **correct
//! and free**: a source that read the set correctly but slower than reading the
//! files one by one would be a worse answer than the one the measurement rules
//! out.
//!
//! # What it reads
//!
//! The set is read as one table, and that is four things:
//!
//! 1. **The metadata is merged.** The files share a schema -- they have to, or
//!    they are not a table -- the rows are the sum, and the row groups are
//!    numbered across the files.
//! 2. **The predicate reaches every file.** The engine's own predicates are
//!    applied to the rows of **every** file, not to the first one that answers.
//! 3. **The batches cross the boundary.** The reader's batch size is derived
//!    from the **merged** row groups, so no row group anywhere ends with a
//!    partial batch, and the rows come back **file by file**, and inside a
//!    file row group by row group.
//! 4. **The answers are the engine's.** The same rows, in the same order, with
//!    the same `NULL` and `NaN` rules, as the same rows in one file.
//!
//! # What it does not do
//!
//! **It does not read the store's manifest.** The embedder hands it the paths,
//! and the store is one way to produce them: the engine does not depend on
//! `columnar-store`, and it never has. **It does not merge schemas** -- files
//! that disagree are refused, and the error says how they disagree. **And it
//! does not read a file that is not Parquet.**

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use arrow::datatypes::{DataType as ArrowType, SchemaRef};

use crate::dataset::{decode_batches_filtered, merge_scans, Dataset, DatasetError, ScanOutput};
use crate::plan::{ColumnSchema, ResolvedFilter};
use crate::pushdown::PushdownPlan;
use crate::source::{
    gcd, largest_divisor_at_most, scalar_cmp, BatchFilter, EngineSource, ParquetSource,
    ReadDecision, RowGroupRange, SourceColumn, MAX_READER_BATCH_ROWS, MIN_READER_BATCH_ROWS,
};
use crate::{EngineError, ExecOptions, Flavor, QueryPlan};

/// How a column's values are stored.
///
/// This is what the **merge** has to agree on: the engine's own merge
/// combines a dictionary-encoded column with a dictionary-encoded one, and a
/// plain column with a plain one, and it refuses a mix -- and a mix that
/// slipped through would drop rows. So the files of a set have to agree about
/// it, and the set says so when they do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Storage {
    /// The values are stored as they are.
    Plain,
    /// The values are stored as indices into a dictionary.
    Dictionary,
}

impl Storage {
    /// How the storage is called, for an error message.
    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Dictionary => "dictionary-encoded",
        }
    }
}

/// A set of Parquet files the engine reads as one table.
///
/// The files are opened once, and their footers are read once: the merged
/// metadata -- the schema, the row count, the row groups and their rows -- is
/// held here, and a read fetches only the columns the plan names.
#[derive(Debug, Clone)]
pub struct FileSetSource {
    /// The files, in the order they were given.
    ///
    /// **The order is the table's order**: the rows come back file by file,
    /// and inside a file row group by row group.
    files: Vec<ParquetSource>,
    /// The table's columns, in schema order.
    columns: Vec<SourceColumn>,
    /// How many rows the table holds.
    rows: usize,
    /// How many row groups the table has, numbered across the files.
    row_groups: usize,
    /// How many rows each of the table's row groups holds.
    row_group_rows: Vec<usize>,
    /// The merged row-group index of each file's first row group.
    ///
    /// The row groups are numbered across the files, so file *f*'s row groups
    /// are `offsets[f]` .. `offsets[f] + files[f].row_groups()`.
    offsets: Vec<usize>,
    /// The smallest and largest value each column holds, over the table.
    ranges: crate::source::ColumnRanges,
    /// The range of each column over each row group, with the row group's rows.
    row_group_ranges: crate::source::RowGroupRanges,
}

impl FileSetSource {
    /// Opens a set of Parquet files as one table.
    ///
    /// The files are opened in the order they are given, and **that order is
    /// the table's order**: the rows come back file by file, and inside a file
    /// row group by row group.
    ///
    /// **The files have to share their schema.** They have to agree about
    /// their columns, their order, their types and how their values are
    /// stored -- a set whose files disagree is not a table, and the error says
    /// which file disagreed and how.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::TableUnreadable`] if a file cannot be opened or
    /// is not a Parquet file this reader understands, and
    /// [`EngineError::SchemaMismatch`] if the files do not share their schema.
    pub fn open(paths: &[PathBuf]) -> Result<Self, EngineError> {
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let source =
                ParquetSource::open(path).map_err(|error| EngineError::TableUnreadable {
                    table: path.display().to_string(),
                    reason: error.to_string(),
                })?;
            files.push(source);
        }
        if files.is_empty() {
            return Err(EngineError::SchemaMismatch(
                "a file set needs at least one file".to_owned(),
            ));
        }
        check_schemas(&files)?;

        let columns = files[0].columns().to_vec();
        let mut row_group_rows = Vec::new();
        let mut offsets = Vec::with_capacity(files.len());
        let mut rows = 0usize;
        let mut row_groups = 0usize;
        for file in &files {
            offsets.push(row_group_rows.len());
            row_group_rows.extend_from_slice(file.row_group_row_counts());
            rows = rows.saturating_add(file.rows());
            row_groups = row_groups.saturating_add(file.row_groups());
        }

        let ranges = merged_ranges(&files);
        let row_group_ranges = merged_row_group_ranges(&files);

        Ok(Self {
            files,
            columns,
            rows,
            row_groups,
            row_group_rows,
            offsets,
            ranges,
            row_group_ranges,
        })
    }

    /// The files, in the table's order.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path().to_path_buf()).collect()
    }

    /// The table's schema: the files' schema.
    ///
    /// The files have to share it -- that is checked when the set is opened --
    /// so the table's schema is the first file's.
    fn schema(&self) -> &SchemaRef {
        self.files[0].schema()
    }

    /// How many rows the table holds.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// How many row groups the table has, numbered across the files.
    #[must_use]
    pub fn row_groups(&self) -> usize {
        self.row_groups
    }

    /// The table's columns, in schema order.
    #[must_use]
    pub fn columns(&self) -> &[SourceColumn] {
        &self.columns
    }

    /// The index of the column called `name`.
    #[must_use]
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column.name == name)
    }

    /// The smallest and largest value a column holds, over the table.
    ///
    /// The range is the **union** of the files' ranges: a column's range over
    /// a set is the table's range, not one file's.
    #[must_use]
    pub fn column_range(&self, name: &str) -> Option<crate::source::ColumnRange> {
        self.ranges.get(name).cloned()
    }

    /// The range of a column over each row group, with the row group's rows.
    ///
    /// The row groups are the table's, numbered across the files, and the
    /// ranges are the files' own.
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

    /// The reader's batch size under a given limit, for the whole table.
    ///
    /// The batch size is the **largest divisor of the merged row groups' row
    /// count** that fits under `limit`, so that no row group ends with a
    /// partial batch -- and because the greatest common divisor divides every
    /// row group's row count, it divides them **across the files** too.
    ///
    /// This is the same rule the one-file read follows, applied to the merged
    /// row groups rather than to one file's.
    #[must_use]
    pub fn batch_rows_under(&self, limit: usize) -> usize {
        if self.row_group_rows.is_empty() {
            return limit;
        }
        let smallest = self.row_group_rows.iter().copied().min().unwrap_or(1).max(1);
        let divisor = self.row_group_rows.iter().copied().fold(0usize, gcd).max(1);
        let aligned = largest_divisor_at_most(divisor, limit);
        if aligned >= MIN_READER_BATCH_ROWS {
            return aligned;
        }
        limit.min(smallest)
    }

    /// The table's row-group count.
    ///
    /// This is the **cap** on the read's parallelism: the read splits the
    /// merged row groups across the threads, so a table with six row groups
    /// cannot be read by more than six threads at a time, however many cores
    /// the machine has.
    #[must_use]
    pub fn worker_count(&self, threads: usize) -> usize {
        threads.min(self.row_groups).max(1)
    }

    /// Decides what a read of this table takes, and what it pushes into the
    /// reader.
    ///
    /// The rules are the engine's **one** set of rules -- the same ones the
    /// one-file read follows -- and they are asked here through the same
    /// function, so the two sources cannot answer differently.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownColumn`] when a predicate names a column
    /// the table does not have.
    pub(crate) fn decide_read(
        &self,
        materialised: &[usize],
        filters: &[ResolvedFilter],
    ) -> Result<ReadDecision, EngineError> {
        crate::source::decide_read(&self.columns, materialised, filters)
    }

    /// Reads the given columns, with the given predicates pushed into the
    /// reader.
    ///
    /// `columns` names the table's root columns to read, in ascending order.
    /// The columns outside it are never fetched from the files.
    ///
    /// The dataset that comes back holds the columns in the order given, only
    /// the rows that survived the pushed-down predicates, and **the table's
    /// rows in the table's order**: file by file, and inside a file row group
    /// by row group.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Io`] if a file cannot be read, and
    /// [`DatasetError::Parquet`] if one of them is not one the reader
    /// understands.
    pub fn read(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        flavor: Flavor,
        threads: usize,
    ) -> Result<Dataset, DatasetError> {
        let output = self.read_scanned(columns, pushdown, None, flavor, threads)?;
        Ok(output.dataset)
    }

    /// Reads the given columns, with the given predicates pushed into the
    /// reader, and hands every batch to `filter` before it copies.
    ///
    /// The filter decides which rows are copied into the dataset, and the rows
    /// it drops are never copied at all. The returned rows are the dataset's
    /// own row numbers, one per output row.
    ///
    /// # Errors
    ///
    /// Returns [`DatasetError::Io`] if a file cannot be read, and
    /// [`DatasetError::Parquet`] if one of them is not one the reader
    /// understands, or if the filter breaks its contract.
    pub fn read_filtered(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: &(dyn BatchFilter + Sync),
        flavor: Flavor,
        threads: usize,
    ) -> Result<ScanOutput, DatasetError> {
        self.read_scanned(columns, pushdown, Some(filter), flavor, threads)
    }

    /// Runs a plan over the table.
    ///
    /// This is the file set's own path into the engine, and it is the same
    /// path the one-file read takes: the plan is resolved against the table's
    /// schema, the scan's decision is the reader's decision, and the stages
    /// are the engine's. **The rows are the table's rows**, and the answer is
    /// the engine's answer.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Memory`] when the plan's estimated working set
    /// exceeds `options.memory_budget`, and the usual resolution errors when
    /// the plan does not fit the table's schema.
    pub fn execute(
        &self,
        plan: &QueryPlan,
        options: &ExecOptions,
    ) -> Result<crate::facade::AggregateResult, EngineError> {
        crate::facade::execute_with(plan, self, options)
    }

    /// Reads the table, with or without a filter.
    fn read_scanned(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        threads: usize,
    ) -> Result<ScanOutput, DatasetError> {
        let workers = self.worker_count(threads);
        // One batch size for the whole table: the one the merged row groups
        // agree on. It divides every row group's row count, in every file.
        let batch_rows = self.batch_rows_under(MAX_READER_BATCH_ROWS);
        if workers == 1 {
            return self.read_in_order(columns, pushdown, filter, flavor, batch_rows);
        }
        self.read_parallel(columns, pushdown, filter, flavor, workers, batch_rows)
    }

    /// Reads every file, in the table's order, and decodes them **in one
    /// pass**.
    ///
    /// The files' readers are **chained**, and the chain is decoded by the
    /// engine's own decoder, so the rows are copied **once**, into the
    /// table's own columns. A merge would copy them twice -- once into each
    /// file's chunk, and once into the merged chunk -- and the copy is the
    /// whole of the difference between this and the one-file read.
    fn read_in_order(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        batch_rows: usize,
    ) -> Result<ScanOutput, DatasetError> {
        let readers = self
            .files
            .iter()
            .map(|file| file.reader(columns, pushdown, None, batch_rows))
            .collect::<Result<Vec<_>, _>>()?;
        let projection = crate::source::scan_projection(columns, pushdown);
        decode_batches_filtered(
            readers.into_iter().flatten(),
            self.schema(),
            columns,
            &projection,
            pushdown,
            flavor,
            filter,
        )
    }

    /// Reads the table with several threads.
    ///
    /// The **merged** row groups are split into contiguous blocks, one per
    /// thread, and the parts are merged **in block order**. A block may cross
    /// a file boundary, and the files it touches are read in the table's
    /// order.
    #[allow(clippy::too_many_arguments)]
    fn read_parallel(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        workers: usize,
        batch_rows: usize,
    ) -> Result<ScanOutput, DatasetError> {
        let per_worker = self.row_groups.div_ceil(workers);
        let blocks: Vec<(usize, usize)> = (0..workers)
            .map(|worker| {
                let start = worker * per_worker;
                let end = (start + per_worker).min(self.row_groups);
                (start, end)
            })
            .filter(|(start, end)| end > start)
            .collect();

        // Each thread writes its own part, and the parts are joined **in block
        // order**. Joining in order is what makes the merged dataset hold the
        // table's rows in the table's order, without any sorting and without
        // any shared mutable state.
        let parts = std::thread::scope(|scope| -> Result<Vec<ScanOutput>, DatasetError> {
            let handles: Vec<_> = blocks
                .iter()
                .map(|&(start, end)| {
                    let source = self;
                    scope.spawn(move || {
                        source.read_block(columns, pushdown, filter, flavor, start, end, batch_rows)
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

    /// Reads the merged row groups from `start` to `end`.
    ///
    /// The range is the table's own numbering, so it may cross a file
    /// boundary: the files it touches are read in the table's order, and each
    /// one is read with the row groups of its own. **The portions are chained
    /// and decoded in one pass**, so the worker's rows are copied once, and
    /// the parts are merged afterwards -- which is what the one-file's
    /// parallel read does too.
    #[allow(clippy::too_many_arguments)]
    fn read_block(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: Option<&(dyn BatchFilter + Sync)>,
        flavor: Flavor,
        start: usize,
        end: usize,
        batch_rows: usize,
    ) -> Result<ScanOutput, DatasetError> {
        let mut readers = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            let offset = self.offsets[index];
            let local_start = start.saturating_sub(offset);
            let local_end =
                end.min(offset.saturating_add(file.row_groups())).saturating_sub(offset);
            if local_end <= local_start {
                continue;
            }
            let groups: Vec<usize> = (local_start..local_end).collect();
            readers.push(file.reader(columns, pushdown, Some(groups), batch_rows)?);
        }
        let projection = crate::source::scan_projection(columns, pushdown);
        decode_batches_filtered(
            readers.into_iter().flatten(),
            self.schema(),
            columns,
            &projection,
            pushdown,
            flavor,
            filter,
        )
    }
}

/// Checks that the files agree about their schema.
///
/// The files are one table, so they have to agree about their columns, their
/// order, their types, and how their values are stored. **The storage matters
/// as much as the type**: the engine's own merge combines a
/// dictionary-encoded column with a dictionary-encoded one and a plain column
/// with a plain one, and a mix would drop rows.
fn check_schemas(files: &[ParquetSource]) -> Result<(), EngineError> {
    let Some(first) = files.first() else {
        return Err(EngineError::SchemaMismatch("a file set needs at least one file".to_owned()));
    };
    for (index, file) in files.iter().enumerate().skip(1) {
        let columns = file.columns();
        if columns.len() != first.columns().len() {
            return Err(EngineError::SchemaMismatch(format!(
                "the file at index {index} holds {} columns and the first holds {}",
                columns.len(),
                first.columns().len()
            )));
        }
        for (position, (left, right)) in first.columns().iter().zip(columns).enumerate() {
            if left.name != right.name {
                return Err(EngineError::SchemaMismatch(format!(
                    "the file at index {index} holds column `{}` at position {position}, and the first holds `{}`",
                    right.name, left.name
                )));
            }
            if left.data_type != right.data_type {
                return Err(EngineError::SchemaMismatch(format!(
                    "the file at index {index} holds column `{}` as {}, and the first holds it as {}",
                    left.name,
                    right.data_type.name(),
                    left.data_type.name()
                )));
            }
            if storage(left) != storage(right) {
                return Err(EngineError::SchemaMismatch(format!(
                    "the file at index {index} stores column `{}` {}, and the first stores it {}",
                    left.name,
                    storage(right).name(),
                    storage(left).name()
                )));
            }
        }
    }
    Ok(())
}

/// How a column's values are stored.
fn storage(column: &SourceColumn) -> Storage {
    match column.arrow_type {
        ArrowType::Dictionary(_, _) => Storage::Dictionary,
        _ => Storage::Plain,
    }
}

/// The range of every column over the table: the **union** of the files'.
///
/// A column's range over a set is the table's range, and not one file's: the
/// range is what the join's build side's direct-addressed table is built
/// over, and a range taken from one file would be a range the other files'
/// rows fall outside.
fn merged_ranges(files: &[ParquetSource]) -> crate::source::ColumnRanges {
    let mut ranges: crate::source::ColumnRanges = HashMap::new();
    for file in files {
        for column in file.columns() {
            let Some(range) = file.column_range(&column.name) else {
                continue;
            };
            ranges
                .entry(column.name.clone())
                .and_modify(|current: &mut crate::source::ColumnRange| {
                    if scalar_cmp(&range.0, &current.0) == Ordering::Less {
                        current.0 = range.0.clone();
                    }
                    if scalar_cmp(&range.1, &current.1) == Ordering::Greater {
                        current.1 = range.1.clone();
                    }
                })
                .or_insert(range);
        }
    }
    ranges
}

/// The range of every column over every row group of the table.
///
/// The row groups are the table's, numbered across the files, and the ranges
/// are the files' own.
fn merged_row_group_ranges(files: &[ParquetSource]) -> crate::source::RowGroupRanges {
    let mut ranges: crate::source::RowGroupRanges = HashMap::new();
    for file in files {
        for column in file.columns() {
            let groups = file.row_group_ranges(&column.name);
            if groups.is_empty() {
                continue;
            }
            ranges.entry(column.name.clone()).or_default().extend_from_slice(groups);
        }
    }
    ranges
}

impl EngineSource for FileSetSource {
    fn rows(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> &[SourceColumn] {
        &self.columns
    }

    fn column_index(&self, name: &str) -> Option<usize> {
        FileSetSource::column_index(self, name)
    }

    fn column_range(&self, name: &str) -> Option<crate::source::ColumnRange> {
        FileSetSource::column_range(self, name)
    }

    fn row_group_ranges(&self, name: &str) -> &[RowGroupRange] {
        FileSetSource::row_group_ranges(self, name)
    }

    fn column_names(&self, indices: &[usize]) -> Vec<String> {
        FileSetSource::column_names(self, indices)
    }

    fn worker_count(&self, threads: usize) -> usize {
        FileSetSource::worker_count(self, threads)
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
        FileSetSource::read(self, columns, pushdown, flavor, threads)
    }

    fn read_filtered(
        &self,
        columns: &[usize],
        pushdown: &PushdownPlan,
        filter: &(dyn BatchFilter + Sync),
        flavor: Flavor,
        threads: usize,
    ) -> Result<ScanOutput, DatasetError> {
        FileSetSource::read_filtered(self, columns, pushdown, filter, flavor, threads)
    }
}

impl ColumnSchema for FileSetSource {
    fn column_count(&self) -> usize {
        self.columns.len()
    }

    fn column_name(&self, index: usize) -> &str {
        self.columns.get(index).map_or("", |column| column.name.as_str())
    }

    fn column_type(&self, index: usize) -> columnar_core::DataType {
        self.columns.get(index).map_or(columnar_core::DataType::Int64, |column| column.data_type)
    }
}
