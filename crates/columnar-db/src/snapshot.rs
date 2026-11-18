//! A snapshot: the database's tables as they were at one moment.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use columnar_engine::{
    col, Aggregate, AggregateResult, Catalog, Dataset, ExecOptions, FileSetSource, PlanBuilder,
    QueryPlan, ScalarValue,
};

use crate::error::{Error, Result};
use crate::readers::Readers;
use crate::row::Rows;
use crate::schema::Schema;
use crate::versions::live_filter;

/// A table as the snapshot pinned it.
#[derive(Debug, Clone)]
pub(crate) struct PinnedTable {
    pub(crate) name: String,
    pub(crate) schema: Schema,
    pub(crate) files: Vec<PathBuf>,
}

/// A snapshot: the database's tables as they were at one moment.
///
/// **A snapshot pins the manifests it read.** The manifest is the table's
/// truth — its files, their rows and their bytes — and the snapshot takes the
/// file list out of it **once**, when the snapshot is taken. **So the file
/// list that manifest named is the table for the life of the snapshot**, and a
/// write that lands afterwards is a write the snapshot does not see.
///
/// **And a reader sees the old file set or the new one, never a mixture.**
/// That is the store's rename-commit made reachable: the commit appends a
/// file and rewrites the manifest by a rename, and the old files are never
/// modified, so a pinned list of old files is a consistent table.
///
/// **And the snapshot is what lets a reader and a writer be in the same
/// process.** The writer holds the database mutably; the reader holds a
/// snapshot, which owns everything it needs. So the reader's view is fixed
/// while the writer writes, and the borrow checker says so.
///
/// **And the snapshot's view is the application's.** The hidden columns are
/// filtered out of every read, the rows that a tombstone retired are
/// filtered out of every read, and a plan that names a hidden column is
/// refused — so the reader sees the application's rows and not the
/// database's bookkeeping.
#[derive(Debug)]
pub struct Snapshot {
    tables: BTreeMap<String, PinnedTable>,
    /// The engine's catalog: every table's name, and the path it resolves to.
    catalog: Catalog,
    /// Which table each pinned path belongs to.
    by_path: BTreeMap<PathBuf, String>,
    /// The retired versions of each table, and when they were found.
    dead: RefCell<BTreeMap<String, BTreeSet<i64>>>,
    /// The readers that are alive.
    readers: Arc<Readers>,
    /// This snapshot's own registration.
    id: u64,
}

impl Snapshot {
    /// A snapshot of the tables the database holds.
    pub(crate) fn new(
        tables: BTreeMap<String, PinnedTable>,
        catalog: Catalog,
        by_path: BTreeMap<PathBuf, String>,
        readers: Arc<Readers>,
        id: u64,
    ) -> Self {
        Self { tables, catalog, by_path, dead: RefCell::new(BTreeMap::new()), readers, id }
    }

    /// The table with that name, as the snapshot pinned it.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<SnapshotTable<'_>> {
        let table = self.tables.get(name)?;
        Some(SnapshotTable { snapshot: self, name: name.to_owned(), table })
    }

    /// The database's table names, in name order.
    #[must_use]
    pub fn tables(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    /// The plan, with the table's live rows, and with its join's build side
    /// resolved to the **whole table**.
    ///
    /// **A table is a set of files, and the engine's builder resolves a name
    /// to one path.** The plan the builder makes is right for a file and half
    /// right for a table, and the half that is wrong is the build side: a
    /// table's rows are in all of its files, and the hash table is built over
    /// all of them at once. **So the database resolves it** — the name the
    /// builder resolved is looked up in the snapshot, and the build side
    /// becomes the pinned file list.
    ///
    /// **And the engine's own join takes a list**: `JoinSpec`'s `build_paths`
    /// is a `Vec`, and several paths are read as one table. So this is the
    /// engine's shape, filled in by the one thing that knows what a table is.
    #[must_use]
    pub fn plan(&self) -> PlanBuilder<'_> {
        PlanBuilder::new(&self.catalog)
    }

    /// The plan the database runs: the application's plan, and the live rows.
    ///
    /// **Three things are added to the plan.** The live filter — `deleted = 0`
    /// and one `row_id != <retired id>` per retired version — so the plan
    /// answers with the rows that are live. The projection — the
    /// application's columns, when the plan named none — so the hidden
    /// columns are never materialised. **And the build side's own live
    /// filter**, so a join's build side holds the live rows and not the
    /// tombstones.
    fn effective(&self, name: &str, plan: &QueryPlan) -> Result<QueryPlan> {
        let Some(table) = self.tables.get(name) else {
            return Err(Error::UnknownTable { name: name.to_owned() });
        };
        check_plan(plan)?;
        let dead = self.dead_of(table)?;
        let mut resolved = plan.clone();
        resolved.filter.extend(live_filter(&dead));
        if resolved.projection.is_empty() {
            resolved.projection = table.schema.names().map(str::to_owned).collect();
        }
        if let Some(join) = &plan.join {
            let mut join = join.clone();
            if let Some(build) = self.build_table(&join.build_paths) {
                join.build_paths.clone_from(&build.files);
                let build_dead = self.dead_of(build)?;
                join.build_filter.extend(live_filter(&build_dead));
            }
            resolved.join = Some(join);
        }
        Ok(resolved)
    }

    /// The table a join's build side names.
    fn build_table(&self, paths: &[PathBuf]) -> Option<&PinnedTable> {
        let first = paths.first()?;
        let name = self.by_path.get(first)?;
        self.tables.get(name)
    }

    /// The retired versions of a table, and when they were found.
    ///
    /// **The set is found once and kept**, because the answer cannot change:
    /// the files are pinned, and a tombstone in a pinned file is a tombstone
    /// for the life of the snapshot.
    fn dead_of(&self, table: &PinnedTable) -> Result<BTreeSet<i64>> {
        if let Some(dead) = self.dead.borrow().get(&table.name) {
            return Ok(dead.clone());
        }
        let dead = crate::versions::retired(&table.files)?;
        self.dead.borrow_mut().insert(table.name.clone(), dead.clone());
        Ok(dead)
    }
}

impl Drop for Snapshot {
    /// **The snapshot gives the files it pinned back**, so a compaction that
    /// was waiting for it can go on.
    fn drop(&mut self) {
        self.readers.release(self.id);
    }
}

/// Refuses a plan that names one of the database's hidden columns.
///
/// **The hidden columns are the database's own**, and a plan that names one
/// is asking the database's bookkeeping a question the application has no
/// business asking. **So it is refused rather than answered.**
fn check_plan(plan: &QueryPlan) -> Result<()> {
    for predicate in &plan.filter {
        check_column(&predicate.column)?;
    }
    for name in &plan.projection {
        check_column(name)?;
    }
    if let Some(aggregate) = &plan.aggregate {
        for name in &aggregate.group_by {
            check_column(name)?;
        }
        for aggregate in &aggregate.aggregates {
            match aggregate {
                Aggregate::Sum { expr } => {
                    for name in expr.columns() {
                        check_column(name)?;
                    }
                }
                Aggregate::Count { column }
                | Aggregate::Min { column }
                | Aggregate::Max { column } => {
                    check_column(column)?;
                }
                Aggregate::CountStar => {}
            }
        }
    }
    if let Some(sort) = &plan.sort {
        check_column(&sort.column)?;
    }
    if let Some(join) = &plan.join {
        check_column(&join.probe_key)?;
        check_column(&join.build_key)?;
        for predicate in &join.build_filter {
            check_column(&predicate.column)?;
        }
    }
    Ok(())
}

/// Refuses a column name that is one of the hidden ones.
fn check_column(name: &str) -> Result<()> {
    if crate::versions::is_hidden(name) {
        return Err(Error::HiddenColumn { column: name.to_owned() });
    }
    Ok(())
}

/// A table, as a snapshot pinned it.
#[derive(Debug)]
pub struct SnapshotTable<'a> {
    snapshot: &'a Snapshot,
    name: String,
    table: &'a PinnedTable,
}

impl SnapshotTable<'_> {
    /// The table's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The table's schema.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.table.schema
    }

    /// The files the snapshot pinned for this table.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.table.files
    }

    /// The engine's plan builder, over the snapshot's catalog.
    ///
    /// **The plan is the engine's**, and the builder is the engine's own: the
    /// database does not have a second query language, and the engine's
    /// surface is not widened to serve it. The builder's catalog holds every
    /// table, so **a join's build side can be named** — and the database
    /// resolves that name to the whole table when the plan runs.
    #[must_use]
    pub fn plan(&self) -> PlanBuilder<'_> {
        self.snapshot.plan()
    }

    /// Runs a plan over the table, and answers with the aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyTable`] when the table has no rows,
    /// [`Error::HiddenColumn`] when the plan names one of the database's
    /// hidden columns, and [`Error::Engine`] when the engine refuses the
    /// plan.
    pub fn execute(&self, plan: &QueryPlan, options: &ExecOptions) -> Result<AggregateResult> {
        if self.table.files.is_empty() {
            return Err(Error::EmptyTable { name: self.name.clone() });
        }
        let source = self.source()?;
        let plan = self.snapshot.effective(&self.name, plan)?;
        Ok(source.execute(&plan, options)?)
    }

    /// Runs a plan over the table, and hands the rows out.
    ///
    /// **The rows come back in the engine's own vocabulary** — a dataset of
    /// column chunks — because the database's values are the engine's values
    /// and a second shape would be a conversion at the boundary.
    ///
    /// # Errors
    ///
    /// Returns [`Error::HiddenColumn`] when the plan names one of the
    /// database's hidden columns, and [`Error::Engine`] when the engine
    /// refuses the plan.
    pub fn execute_rows(&self, plan: &QueryPlan, options: &ExecOptions) -> Result<Dataset> {
        if self.table.files.is_empty() {
            return Ok(Dataset::from_chunks(Vec::new()));
        }
        let source = self.source()?;
        let plan = self.snapshot.effective(&self.name, plan)?;
        Ok(source.execute_rows(&plan, options)?)
    }

    /// Runs a plan over the table, and hands the rows out as [`Row`]s.
    ///
    /// **The rows are the engine's values, in the database's row shape** — one
    /// value per column, a NULL being the absence of one. The columns are
    /// named beside them, so a plan that projected a subset of the table says
    /// which subset it got.
    ///
    /// # Errors
    ///
    /// Returns [`Error::HiddenColumn`] when the plan names one of the
    /// database's hidden columns, [`Error::Engine`] when the engine refuses
    /// the plan, and [`Error::Corrupt`] when a file's rows are not readable.
    pub fn rows(&self, plan: &QueryPlan, options: &ExecOptions) -> Result<Rows> {
        if self.table.files.is_empty() {
            return Ok(Rows::empty(self.schema()));
        }
        let dataset = self.execute_rows(plan, options)?;
        Rows::of(&dataset)
    }

    /// The rows that hold a key.
    ///
    /// **And the lookup is the database's own**: the snapshot's, so it sees one
    /// version of the table and not a mixture — and the rows that come back
    /// are the **live** ones, because the plan carries the live filter and not
    /// only the key's.
    ///
    /// **And the key is the schema's own**: the column the table is looked up
    /// by, and a table whose schema names no key has no point lookup, and the
    /// lookup says so rather than guessing a column.
    ///
    /// **And the lookup is a scan today.** The rows are found by filtering the
    /// table, and the map that will make the lookup cheap is measured before
    /// it is built
    /// ([PERF.md §3am](../../PERF.md#3am-the-indexs-shape-measured-before-it-is-built)).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoKey`] when the table's schema names no key column,
    /// [`Error::ValueType`] when the key's value is not the key column's type,
    /// and the errors of [`Self::rows`].
    pub fn lookup(&self, key: impl Into<ScalarValue>) -> Result<Rows> {
        let Some(key_column) = self.schema().key() else {
            return Err(Error::NoKey { name: self.name.clone() });
        };
        let key = key.into();
        if key.data_type() != key_column.data_type() {
            return Err(Error::ValueType {
                column: key_column.name().to_owned(),
                expected: key_column.data_type().name().to_owned(),
                got: key.data_type().name().to_owned(),
            });
        }
        let plan = self.plan().filter(col(key_column.name()).eq(key)).build()?;
        self.rows(&plan, &ExecOptions::default())
    }

    /// The source the table's files are read through.
    fn source(&self) -> Result<FileSetSource> {
        Ok(FileSetSource::open(&self.table.files)?)
    }
}
