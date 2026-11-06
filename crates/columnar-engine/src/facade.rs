//! The library's face.
//!
//! Every name an embedder needs is declared here, once, and the crate root and
//! the prelude carry the same list. The list is short enough to read in one
//! sitting and closed by one rule: **every type that appears in the public
//! signature of a surface item is on it**.
//!
//! ## What the engine does
//!
//! The engine runs a typed query plan over a Parquet file:
//!
//! ```text
//! scan -> filter -> project -> [join] -> [sort] -> aggregate
//! ```
//!
//! and the order the optimisations run in is pinned:
//!
//! ```text
//! predicate pushdown -> projection pushdown -> late materialization -> join -> sort -> aggregate
//! ```
//!
//! Both are printed in the plan output, so a reader never has to guess what
//! the engine did.
//!
//! ## The three guarantees
//!
//! - **A NULL never matches.** Not in the filter, not in the join, not in a
//!   `!=`. A NaN never matches either. The rules are stricter than SQL's and
//!   than Arrow's own comparisons, and they are the same on every path: the
//!   pushed-down filter and the filter that follows it agree.
//! - **The plan output accounts for the whole query.** Every stage reports
//!   what it was handed, what it produced, and how long it took, and the
//!   attribution says how much of the query's own time fell outside the
//!   stages.
//! - **The answer does not depend on the schedule.** Two runs of the same
//!   plan over the same data produce the same output, in the same order, on
//!   any number of threads.
//!
//! ## The boundaries
//!
//! - **Read-only.** The engine reads Parquet and writes nothing.
//! - **Parquet only.** It opens no transport and speaks no binary protocol.
//!   See [ADR-0005](../../../docs/adr/0005-own-column-chunks.md).
//! - **One inner equi-join.** One build side, one probe side, one key.
//! - **No SQL.** The plan is a Rust struct. A parser is a month of work that
//!   produces no evidence about the part of this project that is under test.
//!
//! ## The trust boundary
//!
//! The engine reads **files an embedder's users provide**, so it says what it
//! reads. It opens no transport and speaks no binary protocol, and the one
//! dependency-policy exemption it carries — the `thrift` advisory — is
//! published with its reason in `deny.toml`. **A related weakness is
//! published too, not hidden**: the compact protocol the reader *does* use
//! allocates the declared length eagerly, bounded by a `u32`, so a hostile
//! file's metadata can ask for four gigabytes before a byte of payload is
//! read. The engine does not fix it — the fix is upstream — and a reader who
//! cannot see it cannot decide whether to read an untrusted file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::memory::{estimate_lazy_read_for, estimate_plan_sort, estimate_resolved};
use crate::pipeline::{scan_stage, BuildSide, Outcome, Output, Pipeline, StreamedJoin};
use crate::scan::{decide_scan, fetched_columns, pushdown_descriptions, streamed_join, ScanReport};
use crate::source::EngineSource;

pub use crate::dataset::{Dataset, DatasetError};
pub use crate::explain::{Attribution, JoinReport, PlanOutput, SortReport, StageStats};
pub use crate::file_set::FileSetSource;
pub use crate::memory::{MemoryBudget, MemoryError};
pub use crate::operators::aggregate::{AggregateResult, AggregateValue, Group};
pub use crate::plan::{Aggregate, AggregateSpec, JoinSpec, QueryPlan, SortSpec};
pub use crate::pushdown::PushdownMetrics;
pub use crate::source::{ParquetSource, RowGroupRange, SourceColumn, MAX_READER_BATCH_ROWS};

// The vocabulary the plan's types are made of. These are `columnar-core`'s
// types, and they are on the surface because the surface's own types name
// them: a predicate holds a `ComparisonOp` and a `ScalarValue`, a dataset's
// chunks hold an `Encoding` and a `NullBitmap`.
pub use columnar_core::{
    ArithOp, ColumnChunk, ComparisonOp, DataType, Encoding, Expr, NullBitmap, Predicate,
    ScalarValue,
};
pub use columnar_kernels::Flavor;

/// Everything that can go wrong while executing a plan.
///
/// The enum is **`#[non_exhaustive]`**: the engine gains a variant in every
/// increment that finds a new way to fail, and an exhaustive match on it
/// would be a breaking change each time. A caller outside this crate matches
/// it with a wildcard arm.
///
/// The two variants the naming layer adds are here too: [`EngineError::UnknownTable`]
/// for a name the catalog does not hold, and [`EngineError::TableUnreadable`]
/// for a table that could not be opened.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The plan's estimated working set does not fit the memory budget.
    ///
    /// The estimate is checked before any data is touched, so a plan that
    /// fails this check has done no work at all.
    #[error("the estimated working set exceeds the query memory budget")]
    Memory(#[from] MemoryError),
    /// The plan names a column the dataset does not have.
    #[error("unknown column `{0}`")]
    UnknownColumn(String),
    /// The plan uses a column with a type the operator cannot handle.
    ///
    /// The engine never casts silently: comparing an `int64` column against a
    /// `float64` threshold is a plan error, not something to paper over.
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    /// The plan is inconsistent: it reads a column the dataset does not have.
    ///
    /// The plan was resolved against one schema and run over another, and the
    /// two disagree. It is a plan error, and it is reported as one: a plan
    /// that cannot be run is refused, not crashed on.
    #[error("invalid plan: {0}")]
    InvalidPlan(String),
    /// The dataset could not be read.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The Parquet file could not be parsed.
    #[error("parquet error: {0}")]
    Parquet(String),
    /// The plan named a table the catalog does not hold.
    ///
    /// The error **names the table** and lists the ones the catalog does
    /// hold, because the fix is a name, and a name is easier to fix when the
    /// error shows the alternatives.
    #[error("unknown table `{name}`{}", known_tables(.known))]
    UnknownTable {
        /// The name the plan used.
        name: String,
        /// The names the catalog holds.
        known: Vec<String>,
    },
    /// A table could not be opened.
    ///
    /// The reason is the reader's own, and the remedy is the same for every
    /// reason: check the path, and check that the file is a Parquet file this
    /// reader understands.
    #[error("could not open table `{table}`: {reason} (check the path, and that the file is a Parquet file)")]
    TableUnreadable {
        /// The table's name, or its path when it was addressed by one.
        table: String,
        /// Why the reader could not open it.
        reason: String,
    },
    /// The files of a set do not share a schema, so they are not one table.
    ///
    /// The error says **which file disagreed and how**, because the remedy is
    /// to make the files agree, and a message that said only "the schemas do
    /// not match" would leave the reader to find out which file it was.
    #[error("the files do not share a schema: {0}")]
    SchemaMismatch(String),
}

impl From<DatasetError> for EngineError {
    fn from(error: DatasetError) -> Self {
        match error {
            DatasetError::Io(io) => Self::Io(io),
            DatasetError::Parquet(message) => Self::Parquet(message),
            DatasetError::UnsupportedType(message) => Self::TypeMismatch(message),
        }
    }
}

impl EngineError {
    /// The error for a name the catalog does not hold.
    fn unknown_table(name: &str, known: Vec<String>) -> Self {
        Self::UnknownTable { name: name.to_owned(), known }
    }

    /// The error for a table the reader could not open.
    fn table_unreadable(table: &str, reason: &DatasetError) -> Self {
        Self::TableUnreadable { table: table.to_owned(), reason: reason.to_string() }
    }
}

/// What the catalog holds, in the message of an unknown table.
///
/// The list is the remedy: the error says which names *are* good, so the
/// reader does not have to go and look.
fn known_tables(known: &[String]) -> String {
    if known.is_empty() {
        return ": the catalog holds no tables; add one with `Catalog::with_table`".to_owned();
    }
    let names = known.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
    format!(": the catalog holds {names}; add one with `Catalog::with_table`")
}

/// Knobs for a query execution.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExecOptions {
    /// The per-query memory budget. Default 256 MiB.
    pub memory_budget: MemoryBudget,
    /// Which kernel flavor to use. `None` means "the fastest available".
    pub flavor: Option<Flavor>,
    /// How many threads to read the file with. `None` means one.
    ///
    /// This is the **read**'s thread count. The operators are single-threaded
    /// in this increment, and the headline comparison pins every system to
    /// one thread so the comparison is like-for-like, so the default is one.
    ///
    /// The read splits the file's **row groups** across the threads -- a row
    /// group is decoded independently of every other one -- and merges the
    /// parts back together afterwards. The rows come back in the file's order
    /// whatever this is set to, so the answer does not depend on the
    /// scheduler.
    pub threads: Option<usize>,
}

/// Executes `plan` over `dataset`.
///
/// The plan is resolved against the dataset first, so an unknown column or a
/// type mismatch is reported before any data is read. The estimated working
/// set is then checked against `options.memory_budget`; if it does not fit,
/// the call returns [`EngineError::Memory`] having done no work.
///
/// Returns [`MemoryError::BudgetExceeded`] if the plan's estimated working
/// set exceeds `options.memory_budget`.
///
/// This is the **eager** path: the dataset already holds every column of the
/// file. The lazy path is [`execute_on`], which reads only what the plan
/// needs.
pub fn execute(
    plan: &QueryPlan,
    dataset: &Dataset,
    options: &ExecOptions,
) -> Result<AggregateResult, EngineError> {
    match execute_output(plan, dataset, options, Output::Aggregate)? {
        Outcome::Aggregate(result) => Ok(*result),
        Outcome::Rows(_) => {
            Err(EngineError::InvalidPlan("the plan's aggregate was not the output".to_owned()))
        }
    }
}

/// Runs a plan and hands its **rows** out.
///
/// This is the write path's way into the engine: the rows a plan produces are
/// the rows a store's table can hold. The rows are the plan's **projection**,
/// gathered -- and a plan with an aggregation is refused, because the
/// aggregate folds the rows into groups and there is nothing left to write.
///
/// The rows come back as a [`Dataset`]: the engine's own vocabulary, a column
/// chunk with an encoding and a validity mask. A store that writes Parquet
/// turns them into Arrow's own.
///
/// # Errors
///
/// Returns [`EngineError::InvalidPlan`] when the plan has an aggregation, and
/// the usual resolution errors when the plan does not fit the dataset's
/// schema.
pub fn execute_rows(
    plan: &QueryPlan,
    dataset: &Dataset,
    options: &ExecOptions,
) -> Result<Dataset, EngineError> {
    match execute_output(plan, dataset, options, Output::Rows)? {
        Outcome::Rows(rows) => Ok(rows),
        Outcome::Aggregate(_) => {
            Err(EngineError::InvalidPlan("the plan's aggregate was not the output".to_owned()))
        }
    }
}

/// Runs a plan over a dataset, for either output.
fn execute_output(
    plan: &QueryPlan,
    dataset: &Dataset,
    options: &ExecOptions,
    output: Output,
) -> Result<Outcome, EngineError> {
    // The clock starts before the plan is resolved: the plan output is the
    // engine's account of the **whole** query, and the plan's resolution is
    // part of it.
    let started = std::time::Instant::now();
    let resolved = plan.resolve(dataset)?;
    refuse_an_aggregate(&resolved, output)?;
    let build = BuildSide::open(resolved.join.as_ref())?;

    let grouped =
        resolved.aggregate.as_ref().is_some_and(|aggregate| !aggregate.group_by.is_empty());
    let estimate = estimate_resolved(&resolved, dataset, grouped)
        .saturating_add(build.estimate(resolved.join.as_ref(), options.threads.unwrap_or(1)))
        .saturating_add(estimate_plan_sort(resolved.sort.as_ref(), dataset.len()));
    if estimate > options.memory_budget.bytes {
        return Err(EngineError::Memory(MemoryError::BudgetExceeded));
    }

    // The eager path read the whole file before the clock started, so the
    // scan's own stage is empty: it cost nothing here.
    let scan = scan_stage(dataset.len(), dataset.len(), std::time::Duration::ZERO);

    let report = ScanReport {
        pushed_down: Vec::new(),
        columns_read: dataset.column_names(),
        pushdown_metrics: PushdownMetrics::default(),
    };

    // The eager path reads the whole file before the plan runs, so the join's
    // probe cannot happen inside the scan: there is no scan to feed.
    Pipeline {
        resolved: &resolved,
        dataset,
        options,
        // The flavor is resolved once, here, and the pipeline runs the same
        // kernels the caller asked for.
        flavor: options.flavor.unwrap_or_else(columnar_kernels::best),
        build: &build,
        started,
        scan,
        report,
        estimate,
        workers: 1,
        streamed: None,
        output,
    }
    .run()
}

/// Refuses a plan that has an aggregation, when the rows are what is wanted.
///
/// The aggregate folds the rows into groups. **There is nothing left to
/// write**: the rows the plan produced are the groups, and a group is not a
/// row of a table.
fn refuse_an_aggregate(
    resolved: &crate::plan::ResolvedPlan,
    output: Output,
) -> Result<(), EngineError> {
    if output == Output::Rows && resolved.aggregate.is_some() {
        return Err(EngineError::InvalidPlan(
            "a plan with an aggregation has no rows to write: the aggregate folds them into groups"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Executes `plan` over a Parquet file, reading only what the plan needs.
///
/// This is the **lazy** path. It differs from [`execute`] in three ways, and
/// each of them is the point:
///
/// - **The plan resolves against the file's schema**, so an unknown column
///   or a type mismatch is reported before a single row is read.
/// - **Only the columns the plan needs are read.** The bytes of the others
///   are never fetched from the file.
/// - **The plan's predicates are pushed down into the scan**, so the rows
///   that fail them are dropped during the scan rather than after it. The
///   engine evaluates them itself and keeps the mask; the reader is given
///   no row filter, because a row filter is a list of row ranges and the
///   list is longer than the mask. See
///   [ADR-0015](../../../docs/adr/0015-the-engine-keeps-the-mask.md).
/// - **The scan copies each surviving row once.** The survivors are written
///   straight out of the reader's array into the engine's own column: the
///   batch is not filtered first, and a column the query does not
///   materialise is never copied. See
///   [ADR-0017](../../../docs/adr/0017-the-scan-filters-only-what-it-keeps.md).
///
/// The pushdown reproduces the engine's filter semantics exactly -- a NULL
/// never matches, a NaN never matches, for every operator including `!=` --
/// which is stricter than both SQL and Arrow's own comparisons. See
/// [ADR-0010](../../../docs/adr/0010-pushdown-equivalence.md).
///
/// # Errors
///
/// Returns [`EngineError::Memory`] when the plan's estimated working set
/// exceeds `options.memory_budget`, and the usual resolution errors when
/// the plan does not fit the file's schema.
pub fn execute_on(
    plan: &QueryPlan,
    source: &ParquetSource,
    options: &ExecOptions,
) -> Result<AggregateResult, EngineError> {
    match execute_on_output(plan, source, options, Output::Aggregate)? {
        Outcome::Aggregate(result) => Ok(*result),
        Outcome::Rows(_) => {
            Err(EngineError::InvalidPlan("the plan's aggregate was not the output".to_owned()))
        }
    }
}

/// Runs a plan over a Parquet file and hands its **rows** out.
///
/// This is the lazy path's way into the write path: only the columns the plan
/// reads are fetched, the plan's **projection** is gathered, and the rows come
/// back as a [`Dataset`].
///
/// A plan with an aggregation is refused, for the same reason the eager path
/// refuses it: the aggregate folds the rows into groups, and there is nothing
/// left to write.
///
/// # Errors
///
/// Returns [`EngineError::InvalidPlan`] when the plan has an aggregation, and
/// the usual resolution errors when the plan does not fit the file's schema.
pub fn execute_rows_on(
    plan: &QueryPlan,
    source: &ParquetSource,
    options: &ExecOptions,
) -> Result<Dataset, EngineError> {
    match execute_on_output(plan, source, options, Output::Rows)? {
        Outcome::Rows(rows) => Ok(rows),
        Outcome::Aggregate(_) => {
            Err(EngineError::InvalidPlan("the plan's aggregate was not the output".to_owned()))
        }
    }
}

/// Runs a plan over a source.
///
/// This is the engine's **one** query path, and it is written once: a file
/// and a set of files are read by the same path, over the same rules, and
/// they differ only in what they read. The set's own entry point is
/// [`FileSetSource::execute`], which comes here.
///
/// # Errors
///
/// Returns [`EngineError::Memory`] when the plan's estimated working set
/// exceeds `options.memory_budget`, and the usual resolution errors when the
/// plan does not fit the source's schema.
pub(crate) fn execute_with<S: EngineSource>(
    plan: &QueryPlan,
    source: &S,
    options: &ExecOptions,
) -> Result<AggregateResult, EngineError> {
    match execute_source(plan, source, options, Output::Aggregate)? {
        Outcome::Aggregate(result) => Ok(*result),
        Outcome::Rows(_) => {
            Err(EngineError::InvalidPlan("the plan's aggregate was not the output".to_owned()))
        }
    }
}

/// Runs a plan over a source and hands its **rows** out.
///
/// This is the source path's way into the write path. A file's rows come out
/// through [`execute_rows_on`]; a **set's** rows come out through
/// [`FileSetSource::execute_rows`](crate::FileSetSource::execute_rows). **Both
/// go through this one function**, so the rows a set hands out are the rows a
/// file hands out — the same plan, the same stages, the same rules.
///
/// # Errors
///
/// Returns [`EngineError::InvalidPlan`] when the plan has an aggregation, and
/// the usual resolution errors when the plan does not fit the source's schema.
pub(crate) fn execute_rows_with<S: EngineSource>(
    plan: &QueryPlan,
    source: &S,
    options: &ExecOptions,
) -> Result<Dataset, EngineError> {
    match execute_source(plan, source, options, Output::Rows)? {
        Outcome::Rows(rows) => Ok(rows),
        Outcome::Aggregate(_) => {
            Err(EngineError::InvalidPlan("the plan's aggregate was not the output".to_owned()))
        }
    }
}

/// Runs a plan over a source, for either output.
pub(crate) fn execute_source<S: EngineSource>(
    plan: &QueryPlan,
    source: &S,
    options: &ExecOptions,
    output: Output,
) -> Result<Outcome, EngineError> {
    // The clock starts before the plan is resolved: the plan output is the
    // engine's account of the **whole** query, and the plan's resolution is
    // part of it.
    let started = std::time::Instant::now();
    let resolved = plan.resolve_against(source)?;
    refuse_an_aggregate(&resolved, output)?;
    let decision = decide_scan(&resolved, source)?;
    let build = BuildSide::open(resolved.join.as_ref())?;

    // The same flavor the filter would use, so the pushdown and the filter
    // run the same kernel over the same values.
    let flavor = options.flavor.unwrap_or_else(columnar_kernels::best);

    // How many threads will *actually* read. The read splits the file's row
    // groups across the threads, so a file with six row groups has six
    // workers however many threads are asked for. The plan output reports
    // this number rather than the requested one, because a plan that claims
    // fourteen threads and used six is a plan that is telling a lie.
    let workers = source.worker_count(options.threads.unwrap_or(1));

    // The budget is checked **before** the read. The estimate is made from
    // the file's schema and its row count, so a plan that does not fit is
    // refused having read nothing at all -- and it accounts for the parallel
    // read's peak, which holds two copies of the read set rather than one.
    // The join's build side and the sort's working set are in the estimate
    // too: a plan that does not fit is refused before any of it is built.
    let grouped =
        resolved.aggregate.as_ref().is_some_and(|aggregate| !aggregate.group_by.is_empty());
    let estimate = estimate_lazy_read_for(source.rows(), source, &decision.read, workers, grouped)
        .saturating_add(build.estimate(resolved.join.as_ref(), options.threads.unwrap_or(1)))
        .saturating_add(estimate_plan_sort(resolved.sort.as_ref(), source.rows()));
    if estimate > options.memory_budget.bytes {
        return Err(EngineError::Memory(MemoryError::BudgetExceeded));
    }

    // Does the scan feed the join?
    //
    // The scan feeds the join when the join's probe can happen **before** the
    // copy: the plan has a join, every predicate was pushed into the reader
    // -- so the filter stage has no work to do -- and the join's key is a
    // type the probe reads. See ADR-0030.
    let streamed = streamed_join(&resolved, &decision, source);

    // The join's table is built **before** the scan when the scan feeds the
    // join: the probe has to happen before the copy, and the probe needs the
    // table.
    let table = build.streamed_table(resolved.join.as_ref(), options, flavor, streamed)?;

    // The clock starts **after** the build table is built: the table is the
    // join's work, and the join's stage reports it.
    let scan_started = std::time::Instant::now();
    let (dataset, joined, filter_elapsed, examined) = match &table {
        Some(table) => {
            let filter = crate::operators::join::JoinFilter::new(&table.table, table.key);
            let output = source.read_filtered(
                &decision.read,
                &decision.pushdown,
                &filter,
                flavor,
                workers,
            )?;
            (output.dataset, Some(output.rows), output.filter_elapsed, output.examined)
        }
        None => (
            source.read(&decision.read, &decision.pushdown, flavor, workers)?,
            None,
            std::time::Duration::ZERO,
            0,
        ),
    };
    let scan_elapsed = scan_started.elapsed();

    // Every index the plan holds is a column of the *file*; the dataset holds
    // only the read set, so the indices have to move.
    let mapping = decision.mapping(source.column_count());
    let effective = resolved.remap(&mapping)?;

    // The filter ran inside the scan, but it is the **join's** work, and the
    // join's stage reports it.
    let scan =
        scan_stage(source.rows(), dataset.len(), scan_elapsed.saturating_sub(filter_elapsed));

    let report = ScanReport {
        pushed_down: pushdown_descriptions(&decision),
        columns_read: source.column_names(&fetched_columns(&decision)),
        pushdown_metrics: decision.pushdown.metrics(),
    };

    let build_elapsed = table.as_ref().map_or(std::time::Duration::ZERO, |table| table.elapsed);
    let build_rows = table.as_ref().map_or(0, |table| table.table.rows());
    let streamed = joined.map(|rows| {
        StreamedJoin::new(
            rows,
            build_elapsed + filter_elapsed,
            build_elapsed,
            filter_elapsed,
            build_rows,
            examined,
        )
    });

    Pipeline {
        resolved: &effective,
        dataset: &dataset,
        options,
        // The same flavor the read used, so the operators run the same
        // kernels over the same values.
        flavor,
        build: &build,
        started,
        scan,
        report,
        estimate,
        workers,
        streamed,
        output,
    }
    .run()
}

/// Runs a plan over a Parquet file, for either output.
fn execute_on_output(
    plan: &QueryPlan,
    source: &ParquetSource,
    options: &ExecOptions,
    output: Output,
) -> Result<Outcome, EngineError> {
    execute_source(plan, source, options, output)
}

/// Opens a Parquet file for the lazy path.
///
/// This is the helper an embedder needs to get from a path to a source: it
/// opens the file, reads its schema, and reports a failure as
/// [`EngineError::TableUnreadable`] rather than as a bare I/O error, because
/// the thing that failed is a **table**, and the user needs to know which.
///
/// The path is anything path-like: a `&str`, a `String`, a `&Path`.
///
/// # Errors
///
/// Returns [`EngineError::TableUnreadable`] when the file cannot be opened
/// or is not a Parquet file this reader understands.
pub fn open_source(path: impl AsRef<Path>) -> Result<ParquetSource, EngineError> {
    let path = path.as_ref();
    ParquetSource::open(path)
        .map_err(|error| EngineError::table_unreadable(&path.display().to_string(), &error))
}

/// Opens a **named** table, through a catalog.
///
/// The name is resolved first, so an unknown name is an
/// [`EngineError::UnknownTable`] that names the table, and a file that cannot
/// be read is an [`EngineError::TableUnreadable`] that names it too.
///
/// # Errors
///
/// Returns [`EngineError::UnknownTable`] when the catalog does not hold the
/// name, and [`EngineError::TableUnreadable`] when the file cannot be opened.
pub fn open_table(catalog: &Catalog, name: &str) -> Result<ParquetSource, EngineError> {
    let path = catalog.path_of(name)?;
    ParquetSource::open(path).map_err(|error| EngineError::table_unreadable(name, &error))
}

/// A table's address: a file the engine reads, or a name the catalog resolves.
///
/// The shape is the repository's own — the harness's `TableAddress` decides
/// whether a table is a `File` or a `Named` one — and the catalog's reference
/// is the same decision with the name resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAddress<'a> {
    /// The table is this file.
    File(&'a Path),
    /// The table has this name, and the catalog resolves it to a path.
    Named(&'a str),
}

/// A set of named tables.
///
/// The catalog is the **embedder's**: the engine never reads one from disk,
/// never mutates one, and never outlives the borrow the embedder gave it.
/// It is a map from a name to a Parquet path, and the plan builder resolves
/// the names in it when the plan is built — before any data is read.
///
/// ```
/// use columnar_engine::prelude::*;
///
/// let catalog = Catalog::new()
///     .with_table("lineitem", "data/lineitem.parquet")
///     .with_table("part", "data/part.parquet");
///
/// assert_eq!(catalog.path("lineitem"), Some(std::path::Path::new("data/lineitem.parquet")));
/// assert!(catalog.path("orders").is_none());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    tables: BTreeMap<String, PathBuf>,
}

impl Catalog {
    /// A catalog with no tables.
    #[must_use]
    pub fn new() -> Self {
        Self { tables: BTreeMap::new() }
    }

    /// A catalog holding one more table.
    ///
    /// A name that is already in the catalog is replaced.
    #[must_use]
    pub fn with_table(mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.tables.insert(name.into(), path.into());
        self
    }

    /// Adds or replaces a table, and returns the path it replaced.
    pub fn insert(&mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Option<PathBuf> {
        self.tables.insert(name.into(), path.into())
    }

    /// The path of a named table, if the catalog holds one.
    #[must_use]
    pub fn path(&self, name: &str) -> Option<&Path> {
        self.tables.get(name).map(PathBuf::as_path)
    }

    /// The path of a named table, or an error that names the table.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownTable`] when the catalog does not hold
    /// the name. The error lists the names it does hold.
    pub fn path_of(&self, name: &str) -> Result<&Path, EngineError> {
        self.path(name).ok_or_else(|| {
            EngineError::unknown_table(name, self.names().map(str::to_owned).collect())
        })
    }

    /// The catalog's table names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.tables.keys().map(String::as_str)
    }

    /// Resolves a table's address to a path.
    ///
    /// A `File` resolves to itself. A `Named` resolves through the catalog,
    /// and an unknown name is an error that names the table.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownTable`] when the catalog does not hold
    /// the name.
    pub fn resolve(&self, table: TableAddress<'_>) -> Result<PathBuf, EngineError> {
        match table {
            TableAddress::File(path) => Ok(path.to_owned()),
            TableAddress::Named(name) => self.path_of(name).map(Path::to_owned),
        }
    }

    /// A plan builder bound to this catalog.
    #[must_use]
    pub fn plan(&self) -> PlanBuilder<'_> {
        PlanBuilder::new(self)
    }
}

/// The join the builder holds until the plan is built.
///
/// The table is still a **reference** here: a name, or a path. It becomes a
/// path when the plan is built, and not before.
#[derive(Debug, Clone)]
struct PendingJoin<'a> {
    table: TableAddress<'a>,
    probe_key: String,
    build_key: String,
    build_filter: Vec<Predicate>,
}

/// A plan under construction.
///
/// **The facade owns the naming and the engine keeps its path.** The builder
/// holds a catalog and a plan's shape; when the plan is **built**, every name
/// it carries is resolved through the catalog, and the result is an ordinary
/// [`QueryPlan`] — the same struct the engine has always taken, with a path
/// where the name was.
///
/// The resolution happens at build time, before any data is read, so an
/// unknown name is an error before the first byte is fetched, and the error
/// names the table.
///
/// ```
/// use columnar_engine::prelude::*;
///
/// let catalog = Catalog::new()
///     .with_table("lineitem", "data/lineitem.parquet")
///     .with_table("part", "data/part.parquet");
///
/// let plan = catalog
///     .plan()
///     .filter(col("l_discount").gt(0.04))
///     .join(TableAddress::Named("part"), "l_partkey", "p_partkey")
///     .aggregate(AggregateSpec {
///         group_by: Vec::new(),
///         aggregates: vec![Aggregate::CountStar],
///     })
///     .build();
///
/// // The build side's name is a path now.
/// let Ok(plan) = plan else { panic!("the catalog holds both names") };
/// assert_eq!(
///     plan.join.expect("the plan has a join").build_paths[0].to_str(),
///     Some("data/part.parquet")
/// );
/// ```
#[derive(Debug, Clone)]
pub struct PlanBuilder<'a> {
    catalog: &'a Catalog,
    filter: Vec<Predicate>,
    projection: Vec<String>,
    aggregate: Option<AggregateSpec>,
    join: Option<PendingJoin<'a>>,
    sort: Option<SortSpec>,
}

impl<'a> PlanBuilder<'a> {
    /// A builder bound to a catalog.
    #[must_use]
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            filter: Vec::new(),
            projection: Vec::new(),
            aggregate: None,
            join: None,
            sort: None,
        }
    }

    /// Adds a predicate.
    ///
    /// Every predicate must hold for a row to survive, and the predicates
    /// are pushed into the scan: the rows that fail them never reach the
    /// operators behind it.
    #[must_use]
    pub fn filter(mut self, predicate: Predicate) -> Self {
        self.filter.push(predicate);
        self
    }

    /// Adds several predicates.
    #[must_use]
    pub fn filters(mut self, predicates: impl IntoIterator<Item = Predicate>) -> Self {
        self.filter.extend(predicates);
        self
    }

    /// Sets the columns to materialise.
    ///
    /// An empty list means every column.
    #[must_use]
    pub fn projection<S: Into<String>>(mut self, columns: impl IntoIterator<Item = S>) -> Self {
        self.projection = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the aggregation.
    #[must_use]
    pub fn aggregate(mut self, spec: AggregateSpec) -> Self {
        self.aggregate = Some(spec);
        self
    }

    /// Sets the join.
    ///
    /// The build side is the **right** table, and it is named here: a
    /// [`TableAddress::Named`] is resolved through the catalog when the plan
    /// is built, and a [`TableAddress::File`] is used as it is.
    #[must_use]
    pub fn join(
        mut self,
        table: TableAddress<'a>,
        probe_key: impl Into<String>,
        build_key: impl Into<String>,
    ) -> Self {
        self.join = Some(PendingJoin {
            table,
            probe_key: probe_key.into(),
            build_key: build_key.into(),
            build_filter: Vec::new(),
        });
        self
    }

    /// Sets the join, with predicates on the build side.
    ///
    /// The build side's predicates are applied **before** the table is
    /// hashed, so they save the work of hashing the rows that fail them.
    #[must_use]
    pub fn join_filtered(
        mut self,
        table: TableAddress<'a>,
        probe_key: impl Into<String>,
        build_key: impl Into<String>,
        filter: impl IntoIterator<Item = Predicate>,
    ) -> Self {
        self.join = Some(PendingJoin {
            table,
            probe_key: probe_key.into(),
            build_key: build_key.into(),
            build_filter: filter.into_iter().collect(),
        });
        self
    }

    /// Sets the sort.
    #[must_use]
    pub fn sort(mut self, spec: SortSpec) -> Self {
        self.sort = Some(spec);
        self
    }

    /// Builds the plan, resolving every name it carries.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownTable`] when the plan names a table the
    /// catalog does not hold. The error names the table and lists the ones
    /// the catalog does hold.
    pub fn build(self) -> Result<QueryPlan, EngineError> {
        let join = match self.join {
            Some(pending) => Some(JoinSpec {
                // The resolution: a name becomes a path here, and nowhere
                // else. Nothing is read, and nothing is opened.
                build_paths: vec![self.catalog.resolve(pending.table)?],
                probe_key: pending.probe_key,
                build_key: pending.build_key,
                build_filter: pending.build_filter,
            }),
            None => None,
        };
        Ok(QueryPlan {
            filter: self.filter,
            projection: self.projection,
            aggregate: self.aggregate,
            join,
            sort: self.sort,
        })
    }
}

/// A column, in the expression builder.
///
/// `col("x")` is a column; the methods on it build the two things a plan is
/// made of — a **predicate** and an **aggregate**.
///
/// ```
/// use columnar_engine::prelude::*;
///
/// let predicate = col("l_discount").gt(0.04);
/// assert_eq!(predicate.op, ComparisonOp::Gt);
///
/// let aggregate = col("l_extendedprice").sum();
/// assert!(matches!(aggregate, Aggregate::Sum { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnExpr {
    name: String,
}

impl ColumnExpr {
    /// The column's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The column is **greater than** the value.
    #[must_use]
    pub fn gt(self, value: impl Into<ScalarValue>) -> Predicate {
        self.compare(ComparisonOp::Gt, value)
    }

    /// The column is **greater than or equal to** the value.
    #[must_use]
    pub fn gte(self, value: impl Into<ScalarValue>) -> Predicate {
        self.compare(ComparisonOp::GtEq, value)
    }

    /// The column is **less than** the value.
    #[must_use]
    pub fn lt(self, value: impl Into<ScalarValue>) -> Predicate {
        self.compare(ComparisonOp::Lt, value)
    }

    /// The column is **less than or equal to** the value.
    #[must_use]
    pub fn lte(self, value: impl Into<ScalarValue>) -> Predicate {
        self.compare(ComparisonOp::LtEq, value)
    }

    /// The column **equals** the value.
    #[must_use]
    pub fn eq(self, value: impl Into<ScalarValue>) -> Predicate {
        self.compare(ComparisonOp::Eq, value)
    }

    /// The column **does not equal** the value.
    ///
    /// A NULL does not equal anything, so it does not match `!=` either.
    #[must_use]
    pub fn ne(self, value: impl Into<ScalarValue>) -> Predicate {
        self.compare(ComparisonOp::Ne, value)
    }

    /// The **sum** of the column.
    #[must_use]
    pub fn sum(self) -> Aggregate {
        Aggregate::Sum { expr: Expr::column(self.name) }
    }

    /// The **count** of the column's non-NULL values.
    #[must_use]
    pub fn count(self) -> Aggregate {
        Aggregate::Count { column: self.name }
    }

    /// The **minimum** of the column, ignoring NULLs.
    #[must_use]
    pub fn min(self) -> Aggregate {
        Aggregate::Min { column: self.name }
    }

    /// The **maximum** of the column, ignoring NULLs.
    #[must_use]
    pub fn max(self) -> Aggregate {
        Aggregate::Max { column: self.name }
    }

    /// The column, as an arithmetic expression.
    #[must_use]
    pub fn expr(self) -> Expr {
        Expr::column(self.name)
    }

    /// A predicate over this column.
    fn compare(self, op: ComparisonOp, value: impl Into<ScalarValue>) -> Predicate {
        Predicate::new(self.name, op, value)
    }
}

/// An operand of an arithmetic expression: a column, or a constant.
///
/// The operand is what makes the arithmetic read the way the query does:
/// `lit(1.0).sub(col("l_discount"))` is `1 - l_discount`, and
/// `col("l_discount").sub(lit(1.0))` is the other one. The order of a
/// subtraction is not a detail.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A column, by name.
    Column(String),
    /// A constant.
    Constant(ScalarValue),
}

impl Operand {
    /// The operand, as an expression.
    #[must_use]
    pub fn expr(self) -> Expr {
        match self {
            Self::Column(name) => Expr::column(name),
            Self::Constant(value) => Expr::constant(value),
        }
    }
}

/// The arithmetic, as the operators the standard library already names.
///
/// The builder's operands are columns and constants, and every combination
/// of the two is an expression, so the four pairs are written once here
/// rather than four times in a method.
macro_rules! arithmetic {
    ($trait:ident, $method:ident, $operator:ident) => {
        impl std::ops::$trait<ColumnExpr> for ColumnExpr {
            type Output = Expr;

            fn $method(self, other: ColumnExpr) -> Expr {
                Expr::binary(ArithOp::$operator, self.expr(), other.expr())
            }
        }

        impl std::ops::$trait<Operand> for ColumnExpr {
            type Output = Expr;

            fn $method(self, other: Operand) -> Expr {
                Expr::binary(ArithOp::$operator, self.expr(), other.expr())
            }
        }

        impl std::ops::$trait<ColumnExpr> for Operand {
            type Output = Expr;

            fn $method(self, other: ColumnExpr) -> Expr {
                Expr::binary(ArithOp::$operator, self.expr(), other.expr())
            }
        }

        impl std::ops::$trait<Operand> for Operand {
            type Output = Expr;

            fn $method(self, other: Operand) -> Expr {
                Expr::binary(ArithOp::$operator, self.expr(), other.expr())
            }
        }
    };
}

arithmetic!(Add, add, Add);
arithmetic!(Sub, sub, Sub);
arithmetic!(Mul, mul, Mul);

impl From<ColumnExpr> for Operand {
    fn from(column: ColumnExpr) -> Self {
        Self::Column(column.name)
    }
}

impl From<ScalarValue> for Operand {
    fn from(value: ScalarValue) -> Self {
        Self::Constant(value)
    }
}

/// Starts an expression with a column.
///
/// ```
/// use columnar_engine::prelude::*;
///
/// let predicate = col("l_discount").gt(0.04);
/// let aggregate = col("l_quantity").sum();
/// ```
#[must_use]
pub fn col(name: impl Into<String>) -> ColumnExpr {
    ColumnExpr { name: name.into() }
}

/// Starts an expression with a constant.
///
/// ```
/// use columnar_engine::prelude::*;
///
/// // 1 - l_discount, in the order the query writes it.
/// let discount = lit(1.0) - col("l_discount");
/// ```
#[must_use]
pub fn lit(value: impl Into<ScalarValue>) -> Operand {
    Operand::Constant(value.into())
}
