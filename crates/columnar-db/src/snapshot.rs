//! A snapshot: the database's tables as they were at one moment.

use std::collections::BTreeMap;
use std::path::PathBuf;

use columnar_engine::{
    AggregateResult, Catalog, Dataset, ExecOptions, FileSetSource, PlanBuilder, QueryPlan,
};

use crate::error::{Error, Result};
use crate::row::Rows;
use crate::schema::Schema;

/// A table as the snapshot pinned it.
#[derive(Debug, Clone)]
pub(crate) struct PinnedTable {
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
#[derive(Debug)]
pub struct Snapshot {
    tables: BTreeMap<String, PinnedTable>,
    /// The engine's catalog: every table's name, and the path it resolves to.
    catalog: Catalog,
    /// Which table each pinned path belongs to.
    by_path: BTreeMap<PathBuf, String>,
}

impl Snapshot {
    /// A snapshot of the tables the database holds.
    pub(crate) fn new(
        tables: BTreeMap<String, PinnedTable>,
        catalog: Catalog,
        by_path: BTreeMap<PathBuf, String>,
    ) -> Self {
        Self { tables, catalog, by_path }
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

    /// The plan, with its join's build side resolved to the **whole table**.
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
    fn resolved(&self, plan: &QueryPlan) -> QueryPlan {
        let mut resolved = plan.clone();
        let Some(join) = &plan.join else {
            return resolved;
        };
        let Some(first) = join.build_paths.first() else {
            return resolved;
        };
        let Some(name) = self.by_path.get(first) else {
            return resolved;
        };
        let Some(table) = self.tables.get(name) else {
            return resolved;
        };
        let mut join = join.clone();
        join.build_paths.clone_from(&table.files);
        resolved.join = Some(join);
        resolved
    }
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
        PlanBuilder::new(&self.snapshot.catalog)
    }

    /// Runs a plan over the table, and answers with the aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyTable`] when the table has no rows, and
    /// [`Error::Engine`] when the engine refuses the plan.
    pub fn execute(&self, plan: &QueryPlan, options: &ExecOptions) -> Result<AggregateResult> {
        if self.table.files.is_empty() {
            return Err(Error::EmptyTable { name: self.name.clone() });
        }
        let source = self.source()?;
        Ok(source.execute(&self.snapshot.resolved(plan), options)?)
    }

    /// Runs a plan over the table, and hands the rows out.
    ///
    /// **The rows come back in the engine's own vocabulary** — a dataset of
    /// column chunks — because the database's values are the engine's values
    /// and a second shape would be a conversion at the boundary.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] when the engine refuses the plan.
    pub fn execute_rows(&self, plan: &QueryPlan, options: &ExecOptions) -> Result<Dataset> {
        if self.table.files.is_empty() {
            return Ok(Dataset::from_chunks(Vec::new()));
        }
        let source = self.source()?;
        Ok(source.execute_rows(&self.snapshot.resolved(plan), options)?)
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
    /// Returns [`Error::Engine`] when the engine refuses the plan, and
    /// [`Error::Corrupt`] when a file's rows are not readable.
    pub fn rows(&self, plan: &QueryPlan, options: &ExecOptions) -> Result<Rows> {
        if self.table.files.is_empty() {
            return Ok(Rows::empty(self.schema()));
        }
        let dataset = self.execute_rows(plan, options)?;
        Rows::of(&dataset)
    }

    /// The source the table's files are read through.
    fn source(&self) -> Result<FileSetSource> {
        Ok(FileSetSource::open(&self.table.files)?)
    }
}
