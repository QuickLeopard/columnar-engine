//! The pipeline: the stages, in the plan's order.
//!
//! `scan -> filter -> project -> [join] -> [sort] -> aggregate` -- the order
//! is pinned, and it is printed at the top of every plan output.
//!
//! The pipeline is the **orchestration**: it holds the state every stage
//! needs, hands each stage what it reads, and assembles the plan output
//! from what the stages report. The stages themselves are the **leaves**:
//! the filter, the projection, the expression, the aggregate, the sort and
//! the join, each one in `operators/`, each one a computation over a
//! dataset.
//!
//! ## The context
//!
//! The stages share a handful of things: the resolved plan, the dataset it
//! runs over, the options, the clock the whole query is measured against, and
//! what the scan and the join's build side decided. [`Pipeline`] carries
//! them, so the pipeline's own entry point takes **one** argument instead of
//! ten, and a new stage is handed the same context as the others.

use std::time::Instant;

use columnar_kernels::Flavor;

use crate::explain::{self, PlanOutput, StageStats};
use crate::operators;
use crate::operators::aggregate::{AggregateResult, Group};
use crate::plan;
use crate::scan::ScanReport;
use crate::source::ParquetSource;
use crate::{Dataset, EngineError, ExecOptions};

/// A stage's report.
///
/// This is the **one** shape a stage reports in: the stage's name, the rows it
/// was handed, the rows it produced, and how long it took. A stage that did
/// not run has no report, and the plan output does not name it.
///
/// The rows are `None` when the stage did not run, and the report is `None`
/// with them: the two travel together, so a stage cannot report a count for a
/// stage that did not happen.
pub(crate) fn stage(
    name: &str,
    rows: Option<(u64, u64)>,
    elapsed: std::time::Duration,
) -> Option<StageStats> {
    rows.map(|(rows_in, rows_out)| StageStats::new(name, rows_in, rows_out, elapsed))
}

/// The scan's own stage report.
///
/// The scan is the first stage, and it always runs, so it always has a
/// report. The report is an `Option` because every stage's report is one: a
/// stage that did not run has none.
///
/// The scan's own numbers are the caller's: how many rows the file held, how
/// many the scan kept, and how long the read took.
pub(crate) fn scan_stage(
    rows_in: usize,
    rows_out: usize,
    elapsed: std::time::Duration,
) -> Option<StageStats> {
    stage("scan", Some((rows(rows_in), rows(rows_out))), elapsed)
}

/// What the pipeline is given to run.
///
/// Every stage reads the same context: the plan, the dataset, the options,
/// the clock, and what the scan and the build side decided. Carrying them
/// in one struct is what lets the pipeline's own function take one argument.
pub(crate) struct Pipeline<'a> {
    /// The plan, resolved.
    pub(crate) resolved: &'a plan::ResolvedPlan,
    /// The dataset the plan runs over.
    pub(crate) dataset: &'a Dataset,
    /// The options the query was given.
    pub(crate) options: &'a ExecOptions,
    /// The kernel flavor the query runs with.
    ///
    /// The flavor is resolved **once**, by the caller, and carried here: the
    /// read and the operators must run the same kernels over the same values,
    /// and a flavor decided twice is a flavor that could be decided twice
    /// differently.
    pub(crate) flavor: Flavor,
    /// The join's build side, when the plan has a join.
    pub(crate) build: &'a BuildSide,
    /// The clock the **whole** query is measured against.
    pub(crate) started: Instant,
    /// The scan's own stage report.
    ///
    /// The scan is the first stage, and it always runs, so it always has a
    /// report. The report is an `Option` because every stage's report is one:
    /// a stage that did not run has none.
    pub(crate) scan: Option<StageStats>,
    /// What the scan did, for the plan output.
    pub(crate) report: ScanReport,
    /// The estimated working set, in bytes.
    pub(crate) estimate: usize,
    /// How many workers the read used.
    pub(crate) workers: usize,
    /// The join's output, when the scan fed the join.
    pub(crate) streamed: Option<StreamedJoin>,
}

/// The join's build side: the file, and how long it took to open it.
///
/// The opening is the join's own work, so it is reported in the join's
/// stage: the plan output accounts for the whole query, and a file opened
/// outside every stage is a hole in that account.
pub(crate) struct BuildSide {
    /// The build side's file, when the plan has a join.
    source: Option<ParquetSource>,
    /// How long it took to open it.
    opened: std::time::Duration,
}

impl BuildSide {
    /// Opens the join's build side, if the plan has a join.
    ///
    /// The build side is a file of its own, and it is read through the
    /// engine's own source -- the same reader settings as any other file.
    pub(crate) fn open(join: Option<&plan::ResolvedJoin>) -> Result<Self, EngineError> {
        let started = std::time::Instant::now();
        let source = match join {
            Some(join) => Some(ParquetSource::open(&join.spec.build_path)?),
            None => None,
        };
        Ok(Self { source, opened: started.elapsed() })
    }

    /// The build side's file, when the plan has a join.
    ///
    /// The estimate and the scan-that-feeds-the-join path both need the file
    /// itself, and neither of them may reach past this type to take it.
    pub(crate) fn source(&self) -> Option<&ParquetSource> {
        self.source.as_ref()
    }
}

/// The join's table, built before the scan when the scan feeds the join.
pub(crate) struct StreamedTable {
    /// The table the probe reads.
    pub(crate) table: operators::join::BuildTable,
    /// The join's key, in the reader's projection.
    pub(crate) key: usize,
    /// How long the table took to build.
    pub(crate) elapsed: std::time::Duration,
}

/// Builds the join's table before the scan, when the scan feeds the join.
///
/// The probe has to happen **before** the copy, and the probe needs the table,
/// so the table is built before the scan rather than during the pipeline. The
/// join's stage still reports it: the plan output accounts for the whole
/// query, and a table built outside every stage would be a hole in it.
///
/// Returns `None` when the scan does not feed the join, and when the plan has
/// no join at all.
pub(crate) fn build_streamed_table(
    resolved: &plan::ResolvedPlan,
    build: &BuildSide,
    source: &ParquetSource,
    options: &ExecOptions,
    flavor: Flavor,
    key: Option<usize>,
) -> Result<Option<StreamedTable>, EngineError> {
    let (Some(key), Some(spec), Some(build_source)) = (key, resolved.join.as_ref(), build.source())
    else {
        return Ok(None);
    };
    // The join's two keys must have the same type, and the check happens
    // before the table is built: a plan error is refused having read
    // nothing at all.
    require_same_key_type(
        source
            .columns()
            .get(spec.probe_key)
            .map_or(columnar_core::DataType::Int64, |column| column.data_type),
        build_key_type(build_source, &spec.spec.build_key),
    )?;
    let started = std::time::Instant::now();
    let table = operators::join::build_table(
        build_source,
        &spec.spec,
        flavor,
        build_source.worker_count(options.threads.unwrap_or(1)),
    )?;
    Ok(Some(StreamedTable { table, key, elapsed: started.elapsed() }))
}

/// A join the scan fed.
///
/// The scan probed the join's key before it copied anything, so the join's
/// output rows are already the dataset's own row numbers.
pub(crate) struct StreamedJoin {
    /// The join's output rows: the dataset's own row numbers, one per output
    /// row.
    rows: Vec<u32>,
    /// The build table's construction, and the probe's cost.
    elapsed: std::time::Duration,
    /// How many build rows the table holds.
    build_rows: usize,
}

impl StreamedJoin {
    /// Builds the report of a join the scan fed.
    ///
    /// The rows are the scan's own output, the elapsed time is the build
    /// table's construction plus the probe's cost, and the build rows are how
    /// many rows the table holds.
    pub(crate) fn new(rows: Vec<u32>, elapsed: std::time::Duration, build_rows: usize) -> Self {
        Self { rows, elapsed, build_rows }
    }
}

/// Runs the pipeline over an already-resolved plan.
///
/// `started` is the clock the **whole** query is measured against: the plan
/// output accounts for the query, so the account starts before the plan is
/// resolved.
impl Pipeline<'_> {
    /// Runs the stages, in the plan's order.
    pub(crate) fn run(self) -> Result<AggregateResult, EngineError> {
        let Pipeline {
            resolved,
            dataset,
            options,
            flavor,
            build,
            started,
            scan,
            report,
            estimate,
            workers,
            streamed,
        } = self;
        // How many workers the operators may use. The filter's blocks are
        // 64-row aligned and the projection's are larger, so the two can end up
        // with different worker counts -- and the plan reports the count that
        // actually ran for each of them, not the count that was asked for.
        let operator_workers = options.threads.unwrap_or(1).max(1);

        // --- filter -----------------------------------------------------------
        // A predicate that was pushed into the reader has already been applied,
        // and its column is not in the dataset. The filters left here are the
        // ones the reader did not take, and the conjunction is separable, so
        // applying them now gives the same answer.
        //
        // A plan with no filter at all does not run the filter: every row
        // survives, and the selection is built here rather than by the filter
        // operator -- the core's own "every row" selection leaves the padding
        // bits of its last word set, and walking it would visit rows past the
        // end of the batch.
        let filter_started = std::time::Instant::now();
        let (selection, filter_stats) = if resolved.filter.is_empty() {
            (columnar_core::SelectionVector::all(dataset.len()), None)
        } else {
            let (selection, stats) = operators::filter::filter_all_with_workers(
                dataset,
                &resolved.filter,
                flavor,
                operator_workers,
            )?;
            (selection, Some(stats))
        };
        let filter_workers = filter_stats.as_ref().map_or(1, |stats| stats.workers);
        let filter_stage = stage(
            "filter",
            filter_stats.map(|stats| (stats.rows_in, stats.rows_out)),
            filter_started.elapsed(),
        );

        // --- join and sort ----------------------------------------------------
        // The two optional stages decide which rows the aggregate reads, so they
        // run before the aggregate's input is materialised. A plan with neither
        // of them takes the plain path, where the aggregate reads the filter's
        // survivors.
        let ordered =
            ordered_stage(resolved, dataset, &selection, build, options, flavor, streamed)?;

        // The materialisation set is built **after** the filter, because it needs
        // the filter's selection: the columns it gathers are the survivors'
        // rows.
        let mut materialised = operators::project::Materialised::with_workers(
            dataset,
            selection,
            options.threads.unwrap_or(1),
        );

        // --- project ----------------------------------------------------------
        // The project stage materialises exactly the columns the rest of the
        // plan reads. A column that is neither filtered nor aggregated is never
        // decoded: that is the late materialisation the plan output reports.
        //
        // A column that only a **global** aggregate reads, and reads as a plain
        // column, is not materialised at all: the aggregate folds it where it
        // lies, over the filter's selection, and the copy this stage would have
        // made is never made. See
        // [ADR-0016](../../../docs/adr/0016-the-aggregate-reads-in-place.md).
        let project_started = std::time::Instant::now();
        let (aggregate_rows, fusable) =
            project_stage(resolved, dataset, &mut materialised, ordered.as_ref());
        // The projection does not change the row count: the join and the sort
        // do, and they report their own counts.
        //
        // The count is the rows the projection was **handed**, which is not
        // always the filter's survivors: behind a join or a sort the projection
        // gathers only the rows those stages decided on. The plan output used to
        // report the survivors' count here, which made a projection that
        // gathered 216,783 rows over six million look like it had gathered six
        // million.
        let survivors = materialised.selection().len();
        let handed = if ordered.is_some() { aggregate_rows } else { survivors };
        let project_stage =
            stage("project", Some((rows(handed), rows(handed))), project_started.elapsed());

        // --- evaluate ---------------------------------------------------------
        // An aggregate over an expression needs the expression's value, and the
        // expression's value is a column. So the expression is evaluated over the
        // rows that survived the filter -- SQL applies WHERE before the
        // aggregate, and so does this -- and the aggregate sums the result.
        //
        // The aggregate operator does not know that the column it is summing was
        // computed rather than read. That is the point: a computed column is a
        // column, so the aggregate's NULL rules, its type rules and its
        // saturation all keep working, and the differential tests that cover them
        // cover this path too.
        let (computed, evaluate_stats) = evaluate_stage(resolved, &materialised, aggregate_rows)?;

        // --- aggregate --------------------------------------------------------
        let (groups, aggregate_stats) = aggregate_stage(
            resolved.aggregate.as_ref(),
            dataset,
            &materialised,
            &computed,
            &fusable,
            aggregate_rows,
        );

        // The stages are reported in the **plan's** order -- scan, filter,
        // project, join, sort, aggregate -- and not in the order the work ran
        // in: the materialisation of the aggregate's input runs after the join
        // and the sort, because they decide which rows it holds.
        //
        // Every stage's report is the same thing: an `Option` that is `None`
        // when the stage did not run. The stages that ran are named, and the
        // ones that did not are not.
        let stages: Vec<StageStats> = [
            scan,
            filter_stage,
            project_stage,
            ordered.as_ref().and_then(|ordered| ordered.join_stage.clone()),
            ordered.as_ref().and_then(|ordered| ordered.sort_stage.clone()),
            evaluate_stats,
            aggregate_stats,
        ]
        .into_iter()
        .flatten()
        .collect();

        let plan_output = PlanOutput {
            optimization_order: plan::OPTIMIZATION_ORDER,
            stages: stages.clone(),
            materialised_columns: materialised.materialised_names(),
            estimated_bytes: estimate,
            budget_bytes: options.memory_budget.bytes,
            kernel_flavor: operators::flavor_name(flavor).to_owned(),
            pushed_down: report.pushed_down,
            columns_read: report.columns_read,
            pushdown_metrics: report.pushdown_metrics,
            threads: workers,
            filter_workers,
            projection_workers: materialised.projection_workers(),
            join: ordered.as_ref().and_then(|ordered| ordered.join.clone()),
            sort: ordered.as_ref().and_then(|ordered| ordered.sort.clone()),
            // The account of the whole query: the stages add up to this, and
            // whatever fell outside them is named rather than hidden.
            attribution: explain::Attribution::of(started.elapsed(), &stages),
        };

        Ok(AggregateResult { groups, stages, plan_output })
    }
}

/// The rows the stages behind the filter read, when a join or a sort decided
/// them.
pub(crate) struct Ordered {
    /// The dataset's own row numbers, in the order the aggregate reads them.
    ///
    /// A row may be named more than once: a join produces one row per match.
    rows: Vec<u32>,
    /// The join's stage statistics, if the plan had a join.
    join_stage: Option<StageStats>,
    /// The sort's stage statistics, if the plan had a sort.
    sort_stage: Option<StageStats>,
    /// What the join did, for the plan output.
    join: Option<explain::JoinReport>,
    /// What the sort did, for the plan output.
    sort: Option<explain::SortReport>,
}

/// Runs the join and the sort, if the plan has them.
///
/// The result is the list of the dataset's rows the aggregate reads, in the
/// order it reads them. A plan with neither stage takes the plain path, and
/// this returns `None`.
pub(crate) fn ordered_stage(
    resolved: &plan::ResolvedPlan,
    dataset: &Dataset,
    selection: &columnar_core::SelectionVector,
    build: &BuildSide,
    options: &ExecOptions,
    flavor: Flavor,
    streamed: Option<StreamedJoin>,
) -> Result<Option<Ordered>, EngineError> {
    if resolved.join.is_none() && resolved.sort.is_none() {
        return Ok(None);
    }

    let join = join_outcome(resolved, dataset, selection, build, options, flavor, streamed)?;
    let sort = sort_outcome(resolved, dataset, selection, join.rows.as_deref())?;

    // The rows the aggregate reads: the sort's output when the plan has a
    // sort, and the join's output when it has a join and no sort. A plan
    // with neither stage returned above.
    let rows = match sort.rows {
        Some(sorted) => sorted,
        None => join.rows.unwrap_or_else(|| selection.collect_rows()),
    };

    Ok(Some(Ordered {
        rows,
        join_stage: join.stage,
        sort_stage: sort.stage,
        join: join.report,
        sort: sort.report,
    }))
}

/// What the join did, and the rows it produced.
pub(crate) struct JoinOutcome {
    /// The rows the join produced, when the plan had a join.
    rows: Option<Vec<u32>>,
    /// The join's stage statistics, when the plan had a join.
    stage: Option<StageStats>,
    /// What the join did, for the plan output.
    report: Option<explain::JoinReport>,
}

/// Runs the join, if the plan has one.
///
/// The survivors' own row numbers in the dataset are what the join works
/// in, so the join's output is a list of the same numbers: there is no
/// second numbering, and no vector of positions that has to be walked to get
/// from one to the other.
///
/// The numbers are **not** collected first. A plan with no filter has six
/// million of them, and a list of them is twenty-four megabytes that the
/// allocator has to hand out for rows the probe reads once. The probe is
/// handed the selection, and reads the rows as it needs them.
pub(crate) fn join_outcome(
    resolved: &plan::ResolvedPlan,
    dataset: &Dataset,
    selection: &columnar_core::SelectionVector,
    build: &BuildSide,
    options: &ExecOptions,
    flavor: Flavor,
    streamed: Option<StreamedJoin>,
) -> Result<JoinOutcome, EngineError> {
    let Some(spec) = &resolved.join else {
        return Ok(JoinOutcome { rows: None, stage: None, report: None });
    };

    if let Some(streamed) = streamed {
        // The scan fed the join: the probe already happened, inside the
        // scan and before the copy, and the rows it produced are the
        // dataset's own row numbers.
        // The build side's file was opened before the scan, and it is the
        // join's own work.
        let stage = stage(
            "join",
            Some((rows(selection.len()), rows(streamed.rows.len()))),
            streamed.elapsed + build.opened,
        );
        let report = explain::JoinReport {
            probe_key: spec.spec.probe_key.clone(),
            build_key: spec.spec.build_key.clone(),
            build_file: file_name(&spec.spec.build_path),
            build_rows: streamed.build_rows,
            output_rows: streamed.rows.len(),
            probe_in_scan: true,
        };
        return Ok(JoinOutcome { rows: Some(streamed.rows), stage, report: Some(report) });
    }

    let started = std::time::Instant::now();
    let source = build.source.as_ref().ok_or_else(|| {
        EngineError::Parquet("the plan has a join but its build side was not opened".to_owned())
    })?;
    require_same_key_type(
        dataset
            .chunks
            .get(spec.probe_key)
            .map_or(columnar_core::DataType::Int64, columnar_core::ColumnChunk::data_type),
        build_key_type(source, &spec.spec.build_key),
    )?;
    let table = operators::join::build_table(
        source,
        &spec.spec,
        flavor,
        source.worker_count(options.threads.unwrap_or(1)),
    )?;
    let keys = key_column(dataset, spec.probe_key)?;
    // The probe reads the survivors' rows as it needs them. A selection
    // that holds every row is the **range**, and the range is handed over
    // as a range: the probe's own loop then keeps its state where it can
    // be kept, and there is no list of row numbers to build.
    let matched = match selection.all_rows() {
        Some(range) => operators::join::probe_rows(&table, &keys, range),
        None => operators::join::probe_rows(&table, &keys, selection.iter()),
    };
    let stage = stage(
        "join",
        Some((rows(selection.len()), rows(matched.len()))),
        // The build side's file was opened before the pipeline ran, and it
        // is the join's own work.
        started.elapsed() + build.opened,
    );
    let report = explain::JoinReport {
        probe_key: spec.spec.probe_key.clone(),
        build_key: spec.spec.build_key.clone(),
        build_file: file_name(&spec.spec.build_path),
        build_rows: table.rows(),
        output_rows: matched.len(),
        probe_in_scan: false,
    };
    Ok(JoinOutcome { rows: Some(matched), stage, report: Some(report) })
}

/// What the sort did, and the rows it kept.
pub(crate) struct SortOutcome {
    /// The sort's output rows, when the plan had a sort.
    rows: Option<Vec<u32>>,
    /// The sort's stage statistics, when the plan had a sort.
    stage: Option<StageStats>,
    /// What the sort did, for the plan output.
    report: Option<explain::SortReport>,
}

/// Runs the sort, if the plan has one.
///
/// The sort is handed the rows the join produced, when there is a join, and
/// the filter's survivors when there is not. Both are the dataset's own row
/// numbers, so the sort's key is one lookup.
///
/// The rows are handed over as they are: a sort with a limit keeps the best
/// `limit` rows and drops the rest, so it never needs more than the heap
/// and the row it is looking at.
pub(crate) fn sort_outcome(
    resolved: &plan::ResolvedPlan,
    dataset: &Dataset,
    selection: &columnar_core::SelectionVector,
    joined: Option<&[u32]>,
) -> Result<SortOutcome, EngineError> {
    let Some(spec) = &resolved.sort else {
        return Ok(SortOutcome { rows: None, stage: None, report: None });
    };

    let started = std::time::Instant::now();
    let keys = key_column(dataset, spec.column)?;
    let handed = joined.map_or(selection.len(), <[u32]>::len);
    // The sort is handed the rows the join produced, when there is a join,
    // and the filter's survivors when there is not. Both are the dataset's
    // own row numbers, so the sort's key is one lookup -- and a selection
    // that holds every row is the range, handed over as a range.
    let sorted = match joined {
        Some(rows) => operators::sort::sort_rows(
            rows.iter().copied(),
            &keys,
            spec.spec.descending,
            spec.spec.limit,
        ),
        None => match selection.all_rows() {
            Some(range) => {
                operators::sort::sort_rows(range, &keys, spec.spec.descending, spec.spec.limit)
            }
            None => operators::sort::sort_rows(
                selection.iter(),
                &keys,
                spec.spec.descending,
                spec.spec.limit,
            ),
        },
    };
    let stage = stage("sort", Some((rows(handed), rows(sorted.len()))), started.elapsed());
    let report = explain::SortReport {
        column: spec.spec.column.clone(),
        descending: spec.spec.descending,
        limit: spec.spec.limit,
        rows: sorted.len(),
    };
    Ok(SortOutcome { rows: Some(sorted), stage, report: Some(report) })
}

/// The column at `index`, read one row at a time.
pub(crate) fn key_column(
    dataset: &Dataset,
    index: usize,
) -> Result<operators::key::KeyColumn<'_>, EngineError> {
    let chunk = dataset.chunks.get(index).ok_or_else(|| {
        EngineError::UnknownColumn("a column the plan reads is not in range".to_owned())
    })?;
    Ok(operators::key::KeyColumn::from_chunk(chunk))
}

/// Rejects a join whose two keys have different types.
///
/// The engine never casts silently: joining an `int64` key to a `utf8` one
/// is a plan error rather than something to paper over.
pub(crate) fn require_same_key_type(
    probe_type: columnar_core::DataType,
    build_type: columnar_core::DataType,
) -> Result<(), EngineError> {
    if probe_type != build_type {
        return Err(EngineError::TypeMismatch(format!(
            "the join's probe key is {} but the build key is {}",
            probe_type.name(),
            build_type.name()
        )));
    }
    Ok(())
}

/// The type of a column of the build side, by name.
pub(crate) fn build_key_type(source: &ParquetSource, build_key: &str) -> columnar_core::DataType {
    source
        .columns()
        .iter()
        .find(|column| column.name == build_key)
        .map_or(columnar_core::DataType::Int64, |column| column.data_type)
}

/// The name of a file, without its directory.
pub(crate) fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// Materialises the columns the stages behind the filter read.
///
/// The plain path gathers the columns over the filter's survivors. The
/// **ordered** path -- the path a join or a sort takes -- gathers them over
/// the rows the join and the sort decided, which is what keeps the
/// aggregate's input down to the rows it actually reads.
///
/// Returns the number of rows the aggregate reads, and the columns it may
/// read where they lie. The ordered path has none: the aggregate reads a
/// list of rows, not a selection, so there is nothing to read in place.
pub(crate) fn project_stage(
    resolved: &plan::ResolvedPlan,
    dataset: &Dataset,
    materialised: &mut operators::project::Materialised<'_>,
    ordered: Option<&Ordered>,
) -> (usize, Vec<usize>) {
    let fusable = match ordered {
        Some(_) => Vec::new(),
        None => fusable_columns(resolved, dataset),
    };
    let aggregate_columns =
        resolved.aggregate.as_ref().map_or_else(Vec::new, plan::ResolvedAggregate::columns);
    for &column in &resolved.materialised {
        if fusable.contains(&column) {
            materialised.note_read_in_place(column);
        } else if ordered.is_some() && !aggregate_columns.contains(&column) {
            // The join's key and the sort's column are read where they lie:
            // the join probes them one row at a time and the sort compares
            // them, and neither needs a copy of the column.
            materialised.note_read_in_place(column);
        } else if let Some(ordered) = ordered {
            materialised.gather_rows(column, &ordered.rows);
        } else {
            materialised.gather(column);
        }
    }
    let rows = match ordered {
        Some(ordered) => ordered.rows.len(),
        None => materialised.selection().len(),
    };
    (rows, fusable)
}

/// Runs the aggregation, and reports the stage.
///
/// A plan with no aggregation has no aggregate stage: it returns no groups and
/// no report, and the plan output does not name a stage that did not run.
pub(crate) fn aggregate_stage(
    aggregate: Option<&plan::ResolvedAggregate>,
    dataset: &Dataset,
    materialised: &operators::project::Materialised<'_>,
    computed: &[Option<columnar_core::BatchColumn>],
    fusable: &[usize],
    aggregate_rows: usize,
) -> (Vec<Group>, Option<StageStats>) {
    let Some(aggregate) = aggregate else {
        return (Vec::new(), None);
    };
    let started = std::time::Instant::now();
    let groups = run_aggregate(aggregate, dataset, materialised, computed, fusable, aggregate_rows);
    let stage =
        stage("aggregate", Some((rows(aggregate_rows), rows(groups.len()))), started.elapsed());
    (groups, stage)
}

/// Runs the aggregation over the rows that survived the filter.
///
/// A **global** aggregation folds the survivors one after another, so it never
/// needs the survivors' values aligned with anything: the columns it reads
/// are read where they lie, and no column of the survivors is built.
///
/// A **grouped** aggregation needs its keys aligned with its values, and the
/// hash table wants both. Every column is gathered there.
pub(crate) fn run_aggregate(
    aggregate: &plan::ResolvedAggregate,
    dataset: &Dataset,
    materialised: &operators::project::Materialised<'_>,
    computed: &[Option<columnar_core::BatchColumn>],
    fusable: &[usize],
    selected_rows: usize,
) -> Vec<Group> {
    if aggregate.group_by.is_empty() {
        let values = global_value_sources(aggregate, dataset, materialised, computed, fusable);
        return operators::aggregate::aggregate_global(&aggregate.spec, &values, selected_rows);
    }

    let group_by: Vec<&columnar_core::BatchColumn> =
        aggregate.group_by.iter().map(|&index| materialised.gathered_ref(index)).collect();
    let values: Vec<Option<&columnar_core::BatchColumn>> = aggregate
        .aggregates
        .iter()
        .zip(computed.iter())
        .map(|(function, computed)| {
            if let plan::ResolvedAggregateFn::Sum { expr } = function {
                if let plan::ResolvedExpr::Column { index, .. } = expr {
                    return Some(materialised.gathered_ref(*index));
                }
                return computed.as_ref();
            }
            function.columns().first().map(|&index| materialised.gathered_ref(index))
        })
        .collect();
    operators::aggregate::aggregate(&aggregate.spec, &group_by, &values, selected_rows)
}

/// The columns the aggregate can read where they lie.
///
/// A column qualifies when three things are true:
///
/// 1. The aggregation is **global** -- one group. A grouped aggregation
///    needs the survivors' values aligned with the survivors' keys, and the
///    gathered column is that alignment.
/// 2. The column is read by a **plain** aggregate -- a sum over the column, a
///    count, a min or a max -- and by nothing else. A column an
///    *expression* reads needs its values aligned with the survivors too,
///    because the expression is evaluated over the survivors.
/// 3. The column's **encoding** can be read one row at a time. A
///    run-length-encoded column holds runs, not values, so reading one of its
///    rows means finding the run the row falls in.
///
/// A column that fails any of the three is gathered, exactly as it always
/// was.
pub(crate) fn fusable_columns(resolved: &plan::ResolvedPlan, dataset: &Dataset) -> Vec<usize> {
    let Some(aggregate) = &resolved.aggregate else {
        return Vec::new();
    };
    if !aggregate.group_by.is_empty() {
        return Vec::new();
    }

    let mut plain: Vec<usize> = Vec::new();
    let mut expression: Vec<usize> = Vec::new();
    for function in &aggregate.aggregates {
        match function {
            plan::ResolvedAggregateFn::Sum { expr } => match expr {
                plan::ResolvedExpr::Column { index, .. } => plain.push(*index),
                other => expression.extend(other.columns()),
            },
            plan::ResolvedAggregateFn::Count { column }
            | plan::ResolvedAggregateFn::Min { column }
            | plan::ResolvedAggregateFn::Max { column } => plain.push(*column),
            plan::ResolvedAggregateFn::CountStar => {}
        }
    }

    // A column an expression reads is gathered, whatever else reads it.
    plain.retain(|column| !expression.contains(column));
    plain.retain(|column| {
        dataset
            .chunks
            .get(*column)
            .and_then(operators::aggregate::SelectedColumn::from_chunk)
            .is_some()
    });
    plain.sort_unstable();
    plain.dedup();
    plain
}

/// The source of each global aggregate's values.
///
/// A column that only a plain aggregate reads, and whose encoding can be read
/// where it lies, is read where it lies: the aggregate folds it over the
/// filter's selection, and the projection never copies it. Everything else is
/// gathered.
pub(crate) fn global_value_sources<'x>(
    aggregate: &plan::ResolvedAggregate,
    dataset: &'x Dataset,
    materialised: &'x operators::project::Materialised<'_>,
    computed: &'x [Option<columnar_core::BatchColumn>],
    fusable: &[usize],
) -> Vec<operators::aggregate::ValueSource<'x>> {
    aggregate
        .aggregates
        .iter()
        .zip(computed.iter())
        .map(|(function, computed)| match function {
            plan::ResolvedAggregateFn::Sum { expr } => match expr {
                plan::ResolvedExpr::Column { index, .. } => {
                    if fusable.contains(index) {
                        selected_source(dataset, materialised, *index)
                    } else {
                        operators::aggregate::ValueSource::Gathered(
                            materialised.gathered_ref(*index),
                        )
                    }
                }
                _ => operators::aggregate::ValueSource::Gathered(
                    computed
                        .as_ref()
                        .expect("an aggregate over an expression has a computed column"),
                ),
            },
            plan::ResolvedAggregateFn::CountStar => operators::aggregate::ValueSource::Rows,
            plan::ResolvedAggregateFn::Count { column }
            | plan::ResolvedAggregateFn::Min { column }
            | plan::ResolvedAggregateFn::Max { column } => {
                if fusable.contains(column) {
                    selected_source(dataset, materialised, *column)
                } else {
                    operators::aggregate::ValueSource::Gathered(materialised.gathered_ref(*column))
                }
            }
        })
        .collect()
}

/// The source of a column the aggregate reads where it lies.
pub(crate) fn selected_source<'x>(
    dataset: &'x Dataset,
    materialised: &'x operators::project::Materialised<'_>,
    index: usize,
) -> operators::aggregate::ValueSource<'x> {
    let chunk = dataset.chunks.get(index).expect("a materialised column index is in range");
    let column = operators::aggregate::SelectedColumn::from_chunk(chunk)
        .expect("a fusable column's encoding can be read where it lies");
    operators::aggregate::ValueSource::Selected {
        column,
        validity: &chunk.validity,
        selection: materialised.selection(),
    }
}

/// Evaluates the aggregates' expressions, and reports the stage.
///
/// A plan whose aggregates all read plain columns has no evaluate stage: the
/// expressions do not need evaluating, and the plan output does not name a
/// stage that did not run.
pub(crate) fn evaluate_stage(
    resolved: &plan::ResolvedPlan,
    materialised: &operators::project::Materialised<'_>,
    aggregate_rows: usize,
) -> Result<(Vec<Option<columnar_core::BatchColumn>>, Option<StageStats>), EngineError> {
    let started = std::time::Instant::now();
    let (computed, evaluated) = evaluate_aggregates(resolved, materialised, aggregate_rows)?;
    if !evaluated {
        return Ok((computed, None));
    }
    let stage =
        stage("evaluate", Some((rows(aggregate_rows), rows(aggregate_rows))), started.elapsed());
    Ok((computed, stage))
}

/// Evaluates the aggregates' expressions over the rows that survived the
/// filter.
///
/// Returns one computed column per aggregate -- `None` for the aggregates
/// that did not need one -- and whether anything was computed at all.
///
/// A sum over a **plain column** needs no evaluation: the column is already
/// materialised, and the aggregate reads it directly. Only a computed
/// expression needs the stage, and only then does the stage appear in the
/// plan output.
pub(crate) fn evaluate_aggregates(
    resolved: &plan::ResolvedPlan,
    materialised: &operators::project::Materialised<'_>,
    rows: usize,
) -> Result<(Vec<Option<columnar_core::BatchColumn>>, bool), EngineError> {
    let mut computed: Vec<Option<columnar_core::BatchColumn>> = Vec::new();
    let mut evaluated = false;
    let Some(aggregate) = &resolved.aggregate else {
        return Ok((computed, evaluated));
    };
    for function in &aggregate.aggregates {
        let value = if let plan::ResolvedAggregateFn::Sum { expr } = function {
            if let plan::ResolvedExpr::Column { .. } = expr {
                None
            } else {
                evaluated = true;
                Some(operators::expr::evaluate(
                    expr,
                    &|index| materialised.gathered_ref(index),
                    rows,
                )?)
            }
        } else {
            None
        };
        computed.push(value);
    }
    Ok((computed, evaluated))
}

/// Converts a row count into the `u64` the statistics use.
pub(crate) fn rows(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}
